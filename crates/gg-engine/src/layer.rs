//! The layer stack: how an app's code, and the engine's tools, plug into the main loop.

use std::any::type_name;
use std::collections::BTreeMap;
#[cfg(feature = "dev")]
use std::collections::HashMap;
use std::fmt;
use std::mem;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "dev")]
use std::time::SystemTime;
use std::time::{Duration, Instant};

use bytemuck::NoUninit;
#[cfg(feature = "dev")]
use gg_api::asset::{AssetId, AssetKind};
use gg_core::math::{DVec3, Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{
    Antialiasing, Blend, Circle, Draws, Grade, Line, MAX_PICKS, Materials, Mesh, Meshes, Mipmaps,
    ModelParams, ModelVertex, Scene, Shader, Shaders, Sprite, SpriteSheet, Streaming, Texels,
    Texture, Textures, Tonemapper, Viewpoint, Viewport,
};
#[cfg(feature = "dev")]
use subsecond::HotFn;
use tracing::{debug, error, info, warn};

use crate::args::{Conditions, Listen, Net, Toward};
#[cfg(feature = "dev")]
use crate::asset_watch::AssetWatch;
use crate::assets::{self, Asset, Assets, Loaded};
#[cfg(feature = "dev")]
use crate::assets::{Catalog, Prints};
use crate::audio::{self, Audio, Bus, Music, Place, Sound};
use crate::camera::{self, Camera, Tie};
use crate::canvas::{self, Canvas};
use crate::cells::{
    self, Cell, CellRun, CellSource, CellState, Entering, Finished, Kept, MadeCells, ModelRead,
    Prep, Prepared, Preparing, Stage, Texts,
};
use crate::dirs;
use crate::draw::{self, Poses, Scratch};
use crate::ecs::text_digest;
use crate::edits::{Authors, Done, Edit};
use crate::environment::{self, Environment};
use crate::font::{self, Align, Font, Fonts, TextStyle};
use crate::game_ui::{self, GameUi};
#[cfg(any(test, feature = "dev"))]
use crate::gltf;
use crate::grid::{self, Plane};
#[cfg(feature = "dev")]
use crate::hot_reload::{Asset as HotAsset, HotReload, Reloaded, Source};
use crate::image::{self, Image};
use crate::interpolation::Past;
use crate::light::{self, Light};
use crate::loader::Loader;
use crate::model::{self, Made, Nodes};
use crate::navigation::{self, NavAgent, NavMeshVolume};
#[cfg(feature = "dev")]
use crate::net::Ticket;
use crate::net::{Build, Code, Outside, Session, SessionStatus, Turn};
use crate::opaque::OpaqueMeshes;
use crate::package::{self, Files};
use crate::perf::{self, Perf};
use crate::physics::{
    self, BoxCollider3D, CapsuleCollider3D, CharacterController3D, CircleCollider2D, Joint2D,
    Joint3D, MeshCollider3D, Physics2D, Physics3D, RigidBody3D, SphereCollider3D,
};
use crate::placed::Placed;
use crate::prefab;
use crate::recording::Recorder;
use crate::recording::text::Text;
use crate::reflect::{Reflect, Value};
use crate::render_thread::Extracted;
use crate::scene_files::{self, SceneFile, SceneFiles};
use crate::skins::Skins;
use crate::snapshot::{self, Again, Run, Snapshot};
use crate::sync_test::SyncTest;
use crate::{
    AnimationStates, Animator, App, BoxCollider2D, Entity, EntityId, Event, Flipbook, Input,
    MeshDraw, Model, ModelNode, ModelRoot, Name, Owner, ParticleEmitter, Prefab, PrefabRoot,
    RigidBody2D, Transform, Tweens, Ui, World,
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

impl Context {
    pub(crate) fn new(window: Option<Window>) -> Self {
        Self::with_world(window, World::default())
    }

    /// A context for `app`'s run, in `window`, if it has one, whose world draws its entities' IDs
    /// from the app's seed, or from a seed drawn at random without one; which decodes the images
    /// of the textures its layers add off the main thread, as [`add_texture`](Self::add_texture)
    /// says; and which plays sound if `aloud`, as a windowed or embedded run does, as
    /// [`add_sound`](Self::add_sound) says.
    pub(crate) fn of_run(window: Option<Window>, app: &App, aloud: bool) -> Self {
        let world = app.seed.map_or_else(World::default, World::seeded);
        let state = window.as_ref().and_then(|_| {
            dirs::dir(app.name, dirs::Kind::State, std::env::consts::OS, |name| {
                std::env::var_os(name)
            })
        });
        let mut cx = Self::with_world(window, world);
        cx.loader = Loader::threaded();
        cx.cells.preparing = Preparing::threaded();
        cx.mods.game = app.name.to_owned();
        cx.mods.dir = (app.mods.clone()).or_else(|| Some(state.as_ref()?.join("mods")));
        cx.state = state;
        if aloud {
            cx.audio = Audio::aloud();
        }
        cx.root.clone_from(&app.root);
        cx.files = app.files.clone();
        cx.start.clone_from(&app.start);
        cx.load_started = app.started;
        cx.own = app.own_sessions.clone();
        #[cfg(feature = "dev")]
        if !cx.reads_package() {
            if let Some(manifest) = app.manifest() {
                cx.folders.clone_from(&manifest.scenes);
                cx.project_dir = manifest.path.parent().map(Path::to_owned);
            }
            cx.asset_cache = (cx.root.as_deref()).and_then(|root| assets::cache(app.name, root));
        }
        cx
    }

    /// Whether the run reads the files under its asset root from its game's package, as
    /// [`App::project`](crate::App::project) says, rather than the disk: such a run reloads none,
    /// and has no asset database, no scene files to open or save and no ID files.
    pub(crate) fn reads_package(&self) -> bool {
        self.files.packaged().is_some()
    }

    /// A context without a window, a renderer or a stack, whose world draws its entities' IDs from
    /// `seed`: for calling a layer's [`attach`](Layer::attach) and its [`tick`](Layer::tick)s
    /// directly, as a test does, so that its simulation goes the same way every run, on every
    /// machine. It decodes each texture's image as it's added, on the calling thread, rather than off
    /// the main thread as a run's context does, but alike otherwise: one whose pixels don't decode
    /// loads and warns, as in the run it goes as.
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
            cx.project_dir = Some(dir.as_ref().to_owned());
        }
        Ok(cx)
    }

    /// Has the layers' [`Mods`](crate::Mods) load the mods in `dir`, as a run given `--mods` does,
    /// for game `game`, as [`App::new`](crate::App::new) names it: for a
    /// [`seeded`](Self::seeded) context.
    #[doc(hidden)]
    #[must_use]
    pub fn mods(mut self, game: &str, dir: impl Into<PathBuf>) -> Self {
        self.mods.game = game.to_owned();
        self.mods.dir = Some(dir.into());
        self
    }

    /// Has the context say it takes part in a networked session, as [`networked`](Self::networked)
    /// does for a session's runs, playing `player`, as [`player`](Self::player) says, `None` for a
    /// server: for a [`seeded`](Self::seeded) context, so that a test calls a layer's `attach` and
    /// `restored` as a session's run does.
    #[doc(hidden)]
    #[must_use]
    pub fn in_session(mut self, player: Option<u8>) -> Self {
        self.networked = true;
        self.player = player;
        self
    }

    /// Has the sessions the layers host and join go through the relay at `relay`, as `--relay`
    /// given alone does for a run: for a [`seeded`](Self::seeded) context, so that a test's layer
    /// starts one as a run's does. No frame takes it up, which [`set_session`](Self::set_session)
    /// stands for.
    #[doc(hidden)]
    #[must_use]
    pub fn with_relay(mut self, relay: &str) -> Self {
        self.own.relay = Some(relay.to_owned());
        self.own.app = "Tested";
        self
    }

    /// Has the context say its run's session stands as `status` says, or that it has none, as a
    /// frame that takes up a session started or left does: [`session`](Self::session),
    /// [`networked`](Self::networked) and [`player`](Self::player) from then on. For a
    /// [`seeded`](Self::seeded) context, so that a test sees its layer follow a session as a run's
    /// does.
    #[doc(hidden)]
    pub fn set_session(&mut self, status: Option<SessionStatus>) {
        self.networked = status.is_some();
        self.player = status.as_ref().map(SessionStatus::player);
        self.session = status;
    }

    fn with_world(window: Option<Window>, mut world: World) -> Self {
        // The engine's names first, so that a game's types can't take them, listed before any
        // entity has one.
        let registered = world.register::<Transform>().and_then(|()| {
            world.register::<Sprite>()?;
            world.register::<MeshDraw>()?;
            world.register::<Camera>()?;
            world.register::<Environment>()?;
            world.register::<Light>()?;
            world.register::<Name>()?;
            world.register::<RigidBody2D>()?;
            world.register::<BoxCollider2D>()?;
            world.register::<Circle>()?;
            world.register::<CircleCollider2D>()?;
            world.register::<RigidBody3D>()?;
            world.register::<BoxCollider3D>()?;
            world.register::<SphereCollider3D>()?;
            world.register::<CapsuleCollider3D>()?;
            world.register::<MeshCollider3D>()?;
            world.register::<CharacterController3D>()?;
            world.register::<Joint2D>()?;
            world.register::<Joint3D>()?;
            world.register::<NavMeshVolume>()?;
            world.register::<NavAgent>()?;
            world.register::<Flipbook>()?;
            world.register::<ParticleEmitter>()?;
            world.register::<Tweens>()?;
            world.register::<Animator>()?;
            world.register::<AnimationStates>()?;
            world.register::<ModelRoot>()?;
            world.register::<ModelNode>()?;
            world.register::<font::Text>()?;
            world.register::<Owner>()?;
            world.register::<PrefabRoot>()?;
            world.register::<crate::Cells>()?;
            world.register::<crate::Viewer>()?;
            world.register::<crate::Streamed>()?;
            world.register_saved::<Physics2D>()?;
            world.register_saved::<Physics3D>()?;
            world.register_saved::<crate::scene_change::SceneIn>()?;
            world.register_saved::<CellState>()
        });
        registered.expect("the engine's components describe themselves");
        let mut textures = Textures::default();
        let fonts = Fonts::new(&mut textures);
        let mut assets = Assets::default();
        assets.add_unnamed(fonts.inter_field());
        let assets = Arc::new(assets);
        world.share_assets(&assets);
        Self {
            window,
            input: Input::default(),
            cursor_grabbed: false,
            max_fps: None,
            clear_color: [0.02, 0.02, 0.02, 1.0],
            antialiasing: None,
            viewpoint: None,
            origin: DVec3::ZERO,
            seen: None,
            seer: None,
            tie: None,
            projection: camera::Projected::default(),
            fog_warned: false,
            environment: None,
            environment_tie: None,
            lit: light::Lit::default(),
            time: 0.0,
            meshes: Meshes::default(),
            shaders: Shaders::default(),
            textures,
            materials: Materials::default(),
            // A test's or a replay's, which draws nothing: on the calling thread, as each is added.
            loader: Loader::inline(),
            streamed: Streaming::default(),
            fonts,
            audio: Audio::default(),
            draws: Draws::default(),
            over: Draws::default(),
            canvas: canvas::Drawn::default(),
            uis: game_ui::Uis::default(),
            colliders: false,
            navmesh: false,
            outline: None,
            outline_root: Vec::new(),
            outline_walk: Vec::new(),
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
            placed: Placed::default(),
            tick_started: 0,
            opaque: OpaqueMeshes::default(),
            skins: Skins::default(),
            scene: Scene::default(),
            attached: false,
            root: None,
            files: Files::default(),
            start: None,
            #[cfg(feature = "dev")]
            folders: Vec::new(),
            #[cfg(feature = "dev")]
            project_dir: None,
            state: None,
            assets,
            loaded: Loaded::default(),
            #[cfg(feature = "dev")]
            given: None,
            follows: false,
            #[cfg(feature = "dev")]
            asset_cache: None,
            #[cfg(feature = "dev")]
            asset_watch: None,
            #[cfg(feature = "dev")]
            hot_reload: None,
            perf: Perf::default(),
            paused: false,
            exiting: false,
            failed: false,
            load_started: None,
            authors: Authors::default(),
            scenes: SceneFiles::default(),
            #[cfg(feature = "dev")]
            scripts: crate::luau::Files::default(),
            hosts: crate::luau::Hosts::default(),
            mods: crate::luau::RunMods::default(),
            #[cfg(feature = "dev")]
            edited: None,
            player: None,
            networked: false,
            own: OwnSessions::default(),
            asked: None,
            session: None,
            copied: None,
            served: false,
            client: false,
            scene_seen: 0,
            refused_seen: 0,
            cells: CellRun::default(),
            #[cfg(feature = "dev")]
            sync_tested: false,
        }
    }

    /// Whether a SyncTest runs each tick again on a copy of the world (`--sync-test`), as
    /// [`Layer::swapped`] says: for the Inspector, whose tick then keeps its log as the tick
    /// started, for the tick's run again.
    #[cfg(feature = "dev")]
    pub(crate) fn sync_tested(&self) -> bool {
        self.sync_tested
    }

    /// The player this run plays, in a lockstep session, which the run hosts (`--host`, or
    /// [`host`](Self::host)), as player 0, or joins (`--join`, or [`join`](Self::join)), as player
    /// 1; `None` outside one, as once a session the game started ends. Each peer runs every
    /// player's world, and its layers queue commands for their own player alone, naming it in
    /// them, as the other
    /// peer's queue for theirs: a command whose variant or struct has a field `player`, an
    /// integer, that names another player than the one whose inputs it came in, is refused there
    /// (`net.bad_command`), by every run of the session alike.
    pub fn player(&self) -> Option<u8> {
        self.player
    }

    /// Whether the run takes part in a networked session: a lockstep session's, as its host
    /// (`--host`) or its joiner (`--join`), or a server's, as the server (`--serve`) or a client
    /// (`--connect`); not a run alone, a replay or a test's context. A game tells by it what only a
    /// run alone has, as a player the game plays itself, which a server's world, whose
    /// [`player`](Self::player) is `None` as a run alone's is, mustn't have. It changes as a
    /// session the game hosts or joins starts, and as it leaves it, from the frame after, which a
    /// layer that sets up such a player follows, as Foxball's does its bot.
    pub fn networked(&self) -> bool {
        self.networked
    }

    /// Hosts a lockstep session through the run's relay, as `--host` given `--relay` alone would,
    /// from the next frame: the run its player 0, as [`player`](Self::player) says from then, and
    /// [`networked`](Self::networked), playing on alone until a run joins by its code, which
    /// [`session`](Self::session) gives, for the game's UI to show. Its peers predict each other's
    /// inputs if the app's sessions do, as [`App::predicting`] says. The run's relay is the one
    /// `--relay` names, given without a session flag, else the one its project's manifest names,
    /// which a build without `dev` reads from its package; the sessions a game starts go through
    /// one, as two players' runs behind their routers need.
    ///
    /// Fails if the run takes part in a session already, or has one started (`net.busy`); names no
    /// relay (`net.no_relay`); is recorded (`net.recorded`), as a recording holds a run alone or a
    /// session from its start; or can't start one, as when its build can't be read (`net.build`),
    /// or its code can't be made (`net.start`). Reads the run's executable, to name its build, as a
    /// session does as it starts: some milliseconds on the calling thread.
    pub fn host(&mut self) -> Result<()> {
        let relay = self.startable()?;
        let code = Code::make().map_err(|error| {
            Error::new("net.start", "can't make the session's code").with_source(error)
        })?;
        let net = Net::Host(Listen {
            address: None,
            relay: Some(relay),
        });
        self.start_session(&net, code)
    }

    /// Joins the lockstep session that `code` names through the run's relay, as `--join` given
    /// `--relay` alone and `--code` would, from the next frame: the run its player 1, as
    /// [`player`](Self::player) says from then, and [`networked`](Self::networked). It runs no tick
    /// until the host's world comes, which takes its own's place, its layers picking up from it as
    /// [`Layer::restored`] says; [`session`](Self::session) says how it goes meanwhile. `code` is
    /// read as a player types it: whatever its case, hyphens and spaces skipped, `I` and `L` read as
    /// `1` and `O` as `0`.
    ///
    /// Fails as [`host`](Self::host) does, and if `code` isn't a session's (`net.code`), saying
    /// why without repeating it, as a code is a session's key.
    pub fn join(&mut self, code: &str) -> Result<()> {
        let relay = self.startable()?;
        let code: Code = code.parse().map_err(|why: String| {
            Error::new("net.code", format!("that isn't a session's code: {why}"))
        })?;
        self.start_session(&Net::Join(Toward::Relay(relay)), code)
    }

    /// The run's relay, for [`host`](Self::host) and [`join`](Self::join), if it can start a
    /// session.
    fn startable(&self) -> Result<String> {
        if self.networked || matches!(self.asked, Some(Asked::Start(_))) {
            let why = "the run takes part in a session already, which it leaves first";
            return Err(Error::new("net.busy", why));
        }
        if self.own.recorded {
            let why = "the run is recorded, and a recording holds a run alone, or a session from \
                       its start";
            return Err(Error::new("net.recorded", why));
        }
        self.own.relay.clone().ok_or_else(|| {
            let why = "the run names no relay, which `--relay` or its project's manifest gives, \
                       for the sessions it starts";
            Error::new("net.no_relay", why)
        })
    }

    /// Starts the session `net` says, keyed by `code`, as the run's own sessions start, for the
    /// stack to take up as the next frame starts.
    fn start_session(&mut self, net: &Net, code: Code) -> Result<()> {
        let own = &self.own;
        let (predicting, link) = (own.predicting, own.link);
        let (app, build, layers) = (own.app, &own.build, &own.layers);
        let session = Session::begin(net, Some(code), app, build, layers, predicting, link)?;
        self.asked = Some(Asked::Start(Box::new(session)));
        Ok(())
    }

    /// Leaves the run's session, if it takes part in one, from the next frame: the run plays on
    /// alone from the world it has, a joiner's its host's, no longer [`networked`](Self::networked),
    /// and its [`player`](Self::player) `None`; the other run loses it, as when a peer's connection
    /// closes. A session started this frame ends before it takes part.
    pub fn leave(&mut self) {
        self.asked = match self.asked.take() {
            // Not taken up yet: it ends as it drops, the threads that find the other run with it.
            Some(Asked::Start(_)) => None,
            _ if self.networked => Some(Asked::Leave),
            asked => asked,
        };
    }

    /// The run's session, as a game's own UI shows it, as [`SessionStatus`] says: as the frame's
    /// turn left it, before its ticks. `None` alone, and for a server or a client, whose sessions a
    /// game's own UI doesn't start.
    pub fn session(&self) -> Option<&SessionStatus> {
        self.session.as_ref()
    }

    /// Puts `text` on the OS's clipboard, as a button that copies does, once the frame ends: in a
    /// windowed run; a headless or embedded run has none, and warns (`ui.clipboard`). A text field
    /// with the focus copies and pastes through it itself, by Ctrl+C, Ctrl+X and Ctrl+V, or Cmd on
    /// macOS.
    pub fn copy(&mut self, text: &str) {
        self.copied = Some(text.to_owned());
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

    /// What the app keeps for the user as file `name`, such as its settings: what
    /// [`save_state`](Self::save_state) last saved there, in a windowed run of the app as
    /// [`App::new`] names it, by this user on this machine; `None` if it saved nothing there, and
    /// in headless runs, which keep nothing for the user, as their dock keeps no layout. `name` is
    /// a file's name, such as `projects.json`, in the OS's directory for the app's state, beside
    /// the dock's layout: on Windows, `%LOCALAPPDATA%\GGEngine\<app>`.
    ///
    /// Fails with `engine.state`, naming the file, if `name` isn't a file's name alone, or if the
    /// file is there but can't be read or isn't a regular file.
    pub fn read_state(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(path) = self.state_path(name)? else {
            return Ok(None);
        };
        match dirs::read_file(&path, u64::MAX) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => {
                let message = format!("can't read what the app kept in {}", path.display());
                Err(Error::new("engine.state", message).with_source(e))
            }
        }
    }

    /// Keeps `data` for the user as file `name`, which [`read_state`](Self::read_state) reads in
    /// the app's later runs, written whole or not at all, so that a crash while it's written
    /// leaves what it held before; and keeps nothing in headless runs.
    ///
    /// Fails with `engine.state`, naming the file, if `name` isn't a file's name alone, or if it
    /// can't be written.
    pub fn save_state(&self, name: &str, data: &[u8]) -> Result<()> {
        let Some(path) = self.state_path(name)? else {
            return Ok(());
        };
        dirs::write(&path, data).map_err(|e| {
            let message = format!("can't keep what the app keeps in {}", path.display());
            Error::new("engine.state", message).with_source(e)
        })
    }

    /// Keeps what the app keeps for the user in `dir`, as a windowed run's context does in the OS's
    /// directory for it, for a test's context.
    #[cfg(all(test, feature = "dev"))]
    pub(crate) fn keep_state_in(&mut self, dir: PathBuf) {
        self.state = Some(dir);
    }

    /// The path of the app's state file `name`, as [`read_state`](Self::read_state) says; `None`
    /// in a run that keeps none.
    fn state_path(&self, name: &str) -> Result<Option<PathBuf>> {
        let alone = Path::new(name).file_name() == Some(std::ffi::OsStr::new(name));
        if !alone {
            let message =
                format!("{name:?} isn't a file's name alone, which the app's state takes");
            return Err(Error::new("engine.state", message));
        }
        Ok(self.state.as_ref().map(|dir| dir.join(name)))
    }

    /// What's held down and where the cursor is, for layers that poll input rather than
    /// follow events.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// Notes what `event` holds down or lets go, as the stack does of each event: for tests that
    /// hand a game's UI its events without a stack.
    #[cfg(test)]
    pub(crate) fn record_input(&mut self, event: &Event<'_>) {
        self.input.record(event);
    }

    /// Whether layers want the cursor grabbed, as [`set_cursor_grabbed`](Self::set_cursor_grabbed)
    /// last set it, whether or not it's grabbed now.
    pub fn cursor_grabbed(&self) -> bool {
        self.cursor_grabbed
    }

    /// Asks for the cursor grabbed, for looking about with the mouse, as first-person games do, or
    /// lets it go. Grabbed, it's hidden and held in the window, locked where it is on Windows,
    /// macOS and Wayland and kept inside the window on X11, so that the mouse's raw motion,
    /// [`Event::MouseMotion`], looks about however far it goes. It's held while the window has
    /// focus, and in a dockspace while the view has the keys, let go as either goes and held again
    /// as it comes back; in a dockspace, a grab that starts with the cursor off the view moves it
    /// to the view's center first, where the platform can move it. Headless runs have no cursor to
    /// grab; a game the editor plays has the editor grab its own window's, while Play gives the
    /// game its input, letting go at Escape or Stop, and grabbing again at a click in the view.
    /// It's applied as the next frame starts, so that the layer that asks last in a frame has its
    /// way.
    pub fn set_cursor_grabbed(&mut self, grabbed: bool) {
        self.cursor_grabbed = grabbed;
    }

    /// The most frames a second a windowed run renders, as
    /// [`set_max_fps`](Self::set_max_fps) last set it; `None` while uncapped.
    pub fn max_fps(&self) -> Option<NonZeroU32> {
        self.max_fps
    }

    /// Caps a windowed run's frames at `max` a second, or uncaps them with `None`, as a game's own
    /// settings menu does: before each frame, the main loop waits out the rest of the frame's
    /// share of a second, counting from when the last was due, so that the rate holds though a
    /// wait ends late, and from the frame's start once frames have fallen a whole share behind, so
    /// that no burst follows a slow one. The simulation's ticks go on in real time however fast
    /// frames come. `--max-fps` sets it as the run starts, and in a development build the title
    /// bar's settings, as [`Ui::settings`](crate::Ui::settings) says, set it and keep it for the
    /// user. Headless runs ignore it, as their frames go as fast as they can or `--real-time` paces
    /// them, and so do embedded ones, which their tool paces by taking frames.
    pub fn set_max_fps(&mut self, max: Option<NonZeroU32>) {
        self.max_fps = max;
    }

    /// The color each frame clears to, in linear RGBA.
    pub fn clear_color(&self) -> [f32; 4] {
        self.clear_color
    }

    /// Sets the color each frame clears to, in linear RGBA.
    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    /// How a layer set the game's view to be antialiased, in place of its world's environment's, if
    /// one has, as [`set_antialiasing`](Self::set_antialiasing) says.
    pub fn antialiasing(&self) -> Option<Antialiasing> {
        self.antialiasing
    }

    /// Has the game's view antialiased as `antialiasing` says, in place of what its world's
    /// [`Environment`] says, a world without one antialiased by it too, from the frame being made
    /// on; or, for `None`, as the environment says again. It's the run's own, as a player's display
    /// settings are, and holds for the run: no hash, scene, snapshot or recording holds it, so that
    /// lockstep peers whose players choose differently, and a replay of either, go as they would.
    pub fn set_antialiasing(&mut self, antialiasing: Option<Antialiasing>) {
        self.antialiasing = antialiasing;
    }

    /// What a layer set the game's view to see the world from this frame, if one has.
    pub fn viewpoint(&self) -> Option<Viewpoint> {
        self.viewpoint
    }

    /// Sets what the game's view sees the world from this frame, in place of the world's camera,
    /// which it sees through in frames no layer sets one: of the entities with a [`Camera`] and a
    /// [`Transform`], the one that's primary; without one, [`Viewpoint::default`]. A projection
    /// that can't project, as [`Projection`](crate::Projection) says, is drawn as the last that
    /// could, or the default's, once for a warning (`engine.bad_projection`) as the frame takes it.
    pub fn set_viewpoint(&mut self, viewpoint: Viewpoint) {
        self.viewpoint = Some(viewpoint);
    }

    /// What every matrix and position a frame draws is relative to, in `f32`, as
    /// [`RenderData::origin`](gg_renderer::RenderData::origin) says: the engine draws the world
    /// from it, and takes what layers draw in the world, by [`draw`](Self::draw),
    /// [`draw_sprite`](Self::draw_sprite) and the rest, from it, so that what's near the view draws
    /// as precisely 1,000 km from the world's origin as at it, but as far as a layer's `f32`s
    /// reach. A multiple of 1,024 on each axis near where the view saw from the frame before, or,
    /// in a development build, where the look's viewpoint is: it moves once the view is more than
    /// 1,024 from it on an axis, to the nearest, so never while the view stays within 1,024 of the
    /// world's origin.
    pub fn origin(&self) -> DVec3 {
        self.origin
    }

    /// Moves the [origin](Self::origin) to follow the look's viewpoint, in a frame that isn't bare,
    /// before the layers draw, so that a frame drawn the moment a tool looks from far off draws
    /// near it as precisely as the frames after.
    #[cfg(feature = "dev")]
    pub(crate) fn follow_look(&mut self) {
        if let Some(looked) = self.look.viewpoint.filter(|_| !self.look.bare) {
            self.origin = followed(self.origin, looked.position);
        }
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
    /// In a development run that serves the tool API, the shader compiles again as its file or a
    /// file it includes changes, and the next frame draws with it; a change that doesn't compile,
    /// or breaks the contract, warns, and the shader keeps drawing as it did.
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
        let file = self.files.file_of(path);
        let added = &self.loaded.shaders;
        let same = |by: &Path| {
            file.as_ref()
                .is_some_and(|file| added.get(by) == Some(file))
        };
        let shader = match self.shaders.find::<V, P>(blend, same) {
            Some(shader) => shader,
            None if self.reads_package() => {
                let spirv = (self.files)
                    .read_as(path, package::Kind::Shader, u64::MAX)
                    .map_err(|error| {
                        let message = format!(
                            "shader {name:?} at {} can't be read from the game's package",
                            path.display()
                        );
                        Error::new("renderer.bad_shader", message).with_source(error)
                    })?;
                self.shaders.add_compiled(name, path, spirv, blend)?
            }
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
    /// (1, 1) at its bottom right. `name` names the texture in errors and GPU captures. The file is
    /// read, and its header, which gives its size, as the texture is added, which hands out its
    /// handle then; its pixels decode off the main thread, and frames stream them to the GPU within
    /// their budget, what samples it sampling a pixel of white until they're there, as
    /// [`streaming`](Self::streaming) tells. One whose header reads but whose pixels don't decode,
    /// as a file cut short past its header, loads all the same, warning `engine.bad_texture` as it
    /// decodes, and samples as white. A run from a game's package reads it as the cooker
    /// compressed it, to BC7. In a development run that serves the tool API, the texture takes its
    /// file's image again as the file changes, at whatever size, once frames have streamed it,
    /// drawing as it did until then, as ARCHITECTURE.md's Rendering says; one that doesn't decode
    /// warns, and the texture keeps drawing as it did. A test's context, [`Context::seeded`],
    /// decodes it on the calling thread as it's added instead, warning at once. A file's add logs
    /// `asset.texture_loaded`, and its pixels' decode `asset.texture_decoded`, each with how long
    /// it took, at the debug level.
    ///
    /// A file added again, by whatever form of its path, gives the texture it gave first, without
    /// loading it again, as ARCHITECTURE.md's Rendering says, and so does an asset's file renamed
    /// or moved since, by the ID that names it; `name` is then unused, and the texture keeps the
    /// path it was first added by, which another path names too; but once the texture's removed,
    /// as [`remove_texture`](Self::remove_texture) says, the file loads anew. Scenes,
    /// snapshots, recordings and the tool API name it by its asset ID, where its file has one, as
    /// ARCHITECTURE.md's Project state says, and otherwise by that path.
    ///
    /// Fails with `engine.bad_texture` if the file can't be read, isn't a regular file, is more
    /// than 1 GiB, isn't a PNG or a JPEG as far as its header, or has a side of more than 8,192
    /// pixels, the most every Vulkan 1.4 device takes, and with `renderer.too_many_textures` past
    /// the 61,440 a renderer holds, the engine's white one among them.
    pub fn add_texture(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Texture> {
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let texture = match self.loaded_texture(&named, path, file.as_deref()) {
            Some((texture, _)) => {
                self.alias(texture, &named);
                texture
            }
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let image = image::load_image(name, path, &self.files)?;
                let size = image.size;
                let (loader, textures) = (&mut self.loader, &mut self.textures);
                let texture = loader.load(textures, name, path, image, Mipmaps::None)?;
                let id = self.texture_name(&named, path, file.as_deref(), None);
                #[cfg(feature = "dev")]
                self.watch_texture(texture, name, path, read_at, &id);
                self.add_assets(|assets| assets.add_texture(texture, named.clone(), id));
                if let Some(file) = file {
                    self.loaded.textures.insert(file, (texture, size));
                }
                texture_loaded(&named, size, started);
                texture
            }
        };
        self.added("texture", &named);
        Ok(texture)
    }

    /// Loads the PNG or JPEG file at `path` as a sprite sheet: a texture, as
    /// [`add_texture`](Self::add_texture) loads one, its pixels decoding off the main thread,
    /// cut into cells `cell` pixels across and down, packed edge to edge from its top left, which
    /// [`SpriteSheet::sprite`] draws, as its header's size places them. A file added before, as a
    /// texture or a sprite sheet, isn't loaded again: its texture is cut into these cells. Its
    /// texture reloads as `add_texture`'s does, but only at the size it has, where its sprites have
    /// its cells. [`remove_texture`](Self::remove_texture) removes its texture, `sheet.texture`.
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
        let file = self.files.file_of(path);
        let sheet = match self.loaded_texture(&named, path, file.as_deref()) {
            Some((texture, size)) => {
                fits(name, path, size, cell)?;
                self.alias(texture, &named);
                SpriteSheet {
                    texture,
                    size,
                    cell,
                }
            }
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let image = image::load_image(name, path, &self.files)?;
                let (loader, textures) = (&mut self.loader, &mut self.textures);
                let sheet = add_sheet(loader, textures, name, path, image, cell)?;
                let (texture, size) = (sheet.texture, sheet.size);
                let id = self.texture_name(&named, path, file.as_deref(), None);
                #[cfg(feature = "dev")]
                self.watch_texture(texture, name, path, read_at, &id);
                self.add_assets(|assets| assets.add_texture(texture, named.clone(), id));
                if let Some(file) = file {
                    self.loaded.textures.insert(file, (texture, size));
                }
                texture_loaded(&named, size, started);
                sheet
            }
        };
        #[cfg(feature = "dev")]
        if !self.loaded.sheets.contains(&sheet.texture) {
            self.loaded.sheets.push(sheet.texture);
        }
        self.added("texture", &named);
        Ok(sheet)
    }

    /// What's still on its way to the GPU, as [`Loading`] says: the textures whose images are
    /// decoding, and what the renderer was streaming once it had rendered the last frame it handed
    /// back. A loading screen shows until it's empty, as a test runs frames until it is.
    pub fn streaming(&self) -> Loading {
        Loading {
            decoding: self.loader.pending(),
            streaming: self.streamed,
        }
    }

    /// When the engine started, once the run has loaded: the first time it's asked once the layers
    /// have attached and nothing is on its way to the GPU, as [`streaming`](Self::streaming) tells,
    /// which the run then logs `engine.loaded` at; `None` before and after, and in a test's or a
    /// replay's context.
    pub(crate) fn take_loaded(&mut self) -> Option<Instant> {
        if self.load_started.is_none() || !self.attached || !self.streaming().is_empty() {
            return None;
        }
        self.load_started.take()
    }

    /// Adds a mesh for layers to draw from then on: triangles, three of `indices` each, which
    /// index `vertices`. A vertex is any `#[repr(C)]` type without padding, which derives
    /// bytemuck's `NoUninit`. `name` names the mesh in errors and GPU captures. Frames stream it
    /// to the GPU within their budget, and draws of it draw nothing until it's there.
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
    /// as entities, as many times as layers like, as a [`ModelRoot`] does wherever it names the
    /// model, as scenes do by its name. Its meshes and images are added once, as
    /// [`add_mesh`](Self::add_mesh) and [`add_texture`](Self::add_texture) add them, each image
    /// read as far as its header as the model is, and decoded off the main thread, with mip levels
    /// made there too, so that one whose pixels don't decode warns and samples as white, as a
    /// texture's does; each draws once frames have streamed it to the GPU. `name` names the model
    /// in errors and GPU captures. Its add logs `asset.model_loaded`, with how long it took and how
    /// many meshes and images it added, whose decodes log as a texture's do. A run from a game's
    /// package reads the model as the cooker cooked it instead, its meshes optimized, with levels
    /// of detail that each draw picks by its size on screen, and its images compressed with their
    /// levels, BC7 of colors or values and BC5 of normal maps, but DDS images' blocks as they are,
    /// as ARCHITECTURE.md's Project state says. A file
    /// added again gives the model it gave first, until it's removed, as
    /// [`remove_model`](Self::remove_model) says.
    ///
    /// Of each node it takes the transform, a matrix or a translation, rotation and scale, and its
    /// light, as KHR_lights_punctual gives it, which the node's entity has as a [`Light`]; of each
    /// primitive its mesh draws, the positions, normals and tangents, the texture coordinates its
    /// material's textures name, and its vertex colors, COLOR_0, as floats or normalized integers,
    /// and its indices, of 8, 16 or 32 bits; and of its material, glTF's metallic-roughness, as
    /// ARCHITECTURE.md's Rendering says: the base color factor and texture, the metallic and
    /// roughness factors and their texture, or, in their place, KHR_materials_pbrSpecularGlossiness's
    /// diffuse factor and texture, specular and glossiness factors and their texture, the normal map
    /// and its scale, the occlusion map and its strength, the emissive factor, times
    /// KHR_materials_emissive_strength's strength, and its texture, each texture's wrapping and
    /// whether it magnifies to the nearest texel, the alpha mode and cutoff, whether it's
    /// double-sided, and whether KHR_materials_unlit makes it unlit.
    /// A primitive without normals draws flat, and a lit material's normal map without tangents
    /// takes MikkTSpace's; a material that isn't double-sided draws its triangles' fronts alone.
    /// Buffers
    /// are in files in the model's folder, named by URIs relative to the model's, read no further
    /// than their byteLength, in base64 data URIs or in a GLB's BIN chunk, and images there or in
    /// buffer views: PNGs or JPEGs, as `add_texture` takes them, or DDS files of BC7, with one level
    /// or their full chain, which a texture's MSFT_texture_dds names in place of its source, told
    /// apart by their bytes rather than by the types the model declares. A file added again gives
    /// the model it gave first. It's
    /// named by its asset ID, where its file has one, or else by its path, as a texture is, and its
    /// images by its name and their places in it, such as `5f3c9a1e07b2d846#0` or
    /// `assets/truck.glb#0`. In a development run that serves the tool API, it takes what its files
    /// hold again as they change, as ARCHITECTURE.md's Rendering says: its meshes and images, and,
    /// unless the run is recorded, in a lockstep session or follows another, its nodes and
    /// materials, which its roots' nodes take.
    ///
    /// Its skins move skinned primitives' vertices by their joints, its nodes in the scene, each
    /// vertex by its first four joints, JOINTS_0 and WEIGHTS_0, as the frame draws them; its
    /// animations are clips, linear, stepped or cubic splines of its nodes' translations, rotations
    /// and scales, which an [`Animator`] on a root plays on its nodes.
    ///
    /// It ignores cameras, animations' channels of morph weights or of nodes outside the scene,
    /// JOINTS_1 and WEIGHTS_1, and the extensions the model uses without requiring them but
    /// those it reads, a material's texture that reads other texture coordinates than its others,
    /// or whose image another texture samples as what it doesn't, as data where it samples colors,
    /// and a primitive's TANGENT that holds numbers that aren't finite, which glTF doesn't allow,
    /// drawing the primitive as one without them, and logs which it has at info level, as
    /// `engine.model_ignored`: the model draws as
    /// it is at rest. It ignores without a word the textures' minification filters, since it
    /// samples them trilinearly and anisotropically unless they magnify to the nearest texel, morph
    /// targets while their weights are 0, the other scenes, and whatever the scene doesn't reach,
    /// which it doesn't read.
    ///
    /// Fails, adding nothing, with `engine.unsupported_model` for glTF the engine doesn't draw:
    /// glTF 1, a model without a scene, primitives that aren't triangle lists, sparse accessors and
    /// accessors without a buffer view, morph weights other than 0, a sampler that
    /// wraps across and down differently, a texture without an image, any required extension but
    /// KHR_materials_unlit, KHR_lights_punctual, KHR_materials_emissive_strength,
    /// KHR_materials_pbrSpecularGlossiness and MSFT_texture_dds, and a model that takes more than 8
    /// GiB to read, of its buffers, images, as read and as decoded, vertices and nodes; with
    /// `engine.bad_model` for a file that can't be read, isn't a regular file or isn't
    /// glTF, as far as reading it checks, such as an accessor past its buffer view, an index past
    /// its vertices, a skinned primitive without JOINTS_0 and WEIGHTS_0 or moved by a joint past
    /// its skin's, a skin's joint outside the scene, an animation's times that don't go up from 0,
    /// a node with two parents, a matrix that isn't a translation, rotation and
    /// scale, a light the extension doesn't allow, a URI that's neither a data URI nor a relative
    /// path, or that has a `..`, or an image that isn't a PNG, a JPEG or a DDS of BC7 as far as its
    /// header, and,
    /// though glTF allows them, integers written as floats, such as `1.0`, and a primitive whose
    /// positions span more than its packed vertices reach with what an f32 holds, as -f32::MAX
    /// beside 0 do; and with
    /// `renderer.too_many_textures` if the renderer hasn't room for its images.
    pub fn add_model(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Model> {
        let zone = gg_core::zone!();
        let path = path.as_ref();
        zone.text(path.display());
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let model = match self.loaded_model(&named, path, file.as_deref()) {
            Some(model) => {
                if self.assets.live_model(&named).is_none() {
                    self.add_assets(|assets| assets.alias_model(model, named.clone()));
                }
                model
            }
            None => {
                let started = Instant::now();
                // As a cell's preparing read it on a thread, if it did.
                let read = self.cells.preparing.models.remove(path);
                let read = read.unwrap_or_else(|| ModelRead::now(name, path, &self.files));
                #[cfg(feature = "dev")]
                let read_at = read.at;
                let (imported, files) = read.read?;
                if !imported.ignored.is_empty() {
                    info!(
                        code = "engine.model_ignored",
                        model = name,
                        path = %path.display(),
                        "Model {name:?} draws without what the engine ignores of it: {}",
                        imported.ignored.join(", ")
                    );
                }
                let stores = model::Stores {
                    meshes: &mut self.meshes,
                    shaders: &mut self.shaders,
                    textures: &mut self.textures,
                    materials: &mut self.materials,
                };
                let added = model::add(name, path, imported, stores, &mut self.loader);
                let (nodes, made) = added?;
                let id = self.texture_name(&named, path, file.as_deref(), None);
                // Each named by the model's ID, or path, and its order in the model.
                let images: Vec<_> = (made.images.iter().enumerate())
                    .map(|(i, &(image, _))| {
                        let at = Some(i);
                        let id = self.texture_name(&named, path, file.as_deref(), at);
                        (image, format!("{named}#{i}").into(), id)
                    })
                    .collect();
                let model = self.add_assets(|assets| {
                    for (image, path, id) in images {
                        assets.add_texture(image, path, id);
                    }
                    assets.add_model(named.clone(), id, nodes)
                });
                // Its images are still decoding, each logging as it's done.
                info!(
                    code = "asset.model_loaded",
                    model = &*named,
                    meshes = made.meshes.len(),
                    images = made.images.len(),
                    ms = perf::millis(started.elapsed()),
                    "Model loaded"
                );
                self.loaded.made.push(made);
                if let Some(file) = file {
                    #[cfg(feature = "dev")]
                    self.watch_model(&file, name, path, files, read_at);
                    self.loaded.models.insert(file, model);
                }
                #[cfg(not(feature = "dev"))]
                let _ = files;
                model
            }
        };
        self.added("model", &format!("{named} {}", model_types()));
        Ok(model)
    }

    /// Removes `texture`, a texture or a sprite sheet's (`sheet.texture`) that a layer added or
    /// text named, as a game does with what it draws no more, such as a level's once it has left
    /// the level: the GPU lets go of its image once the frames and copies that may use it have
    /// finished, what of it was still decoding or streaming is dropped, and what samples it samples
    /// a pixel of white from the frame that takes the removal on. Its handle is never given out
    /// again, so that a stale one, in the world or a snapshot, never samples another texture; and
    /// a development run watches its file no more. Removing one removed already does nothing.
    ///
    /// Removal changes nothing the simulation reads: the texture keeps its name and its path, which
    /// the world's hash, scenes and snapshots read, and its names go on finding it, so that the
    /// world's sprites keep it, scene text that a tick loads finds it, and a world hashes alike
    /// whenever a layer removed it. But what loads a texture by its name or its file, a layer's
    /// add, a scene that the tool API or a scene file loads, a recorded or a lockstep peer's
    /// command or edit, and a restore, takes it for none, and loads its file anew, as another
    /// texture, which its names find from then on.
    ///
    /// # Panics
    ///
    /// If `texture` is the engine's white texture or a font's field, which aren't removed, or a
    /// model's image, which goes with its model, as [`remove_model`](Self::remove_model) says; or
    /// another context's.
    pub fn remove_texture(&mut self, texture: Texture) {
        if let Some(why) = self.unremovable(texture) {
            panic!("texture {} {why}", texture.index());
        }
        self.drop_texture(texture, true);
    }

    /// Removes `mesh`, which a layer added, as [`remove_texture`](Self::remove_texture) removes a
    /// texture: the GPU lets go of its buffers once the frames and copies that may use them have
    /// finished, and every draw of it draws nothing from the frame that takes the removal on, its
    /// handle never given out again. The simulation reads nothing of it, as the world's hash reads
    /// nothing of a [`MeshDraw`]. Removing one removed already does nothing.
    ///
    /// # Panics
    ///
    /// If `mesh` is the engine's quad, which sprites, circles, lines and text are drawn on, or a
    /// model's, which goes with its model, as [`remove_model`](Self::remove_model) says; or
    /// another context's.
    pub fn remove_mesh<V>(&mut self, mesh: Mesh<V>) {
        if let Some(why) = self.unremovable_mesh(mesh.index()) {
            panic!("mesh {} {why}", mesh.index());
        }
        self.drop_mesh(mesh.index());
    }

    /// Removes `model`, which a layer added or text named, and its meshes and images with it, as
    /// [`remove_mesh`](Self::remove_mesh) and [`remove_texture`](Self::remove_texture) remove them,
    /// so that what draws them draws nothing, or samples white, from the frame that takes the
    /// removal on; a development run watches its files no more. It keeps its name, its path and
    /// its nodes, as a texture removed keeps its name, so that a root that names it spawns what it
    /// did, whenever it was removed, of draws that draw nothing; but what loads it by its name or
    /// its file loads it anew, as another model, as for a texture. Removing one removed already
    /// does nothing.
    ///
    /// # Panics
    ///
    /// If `model` is another context's.
    pub fn remove_model(&mut self, model: Model) {
        let Some(made) = self.loaded.made.get(model.index()) else {
            panic!("model {} is another context's", model.index());
        };
        if self.assets.model_removed(model) {
            return;
        }
        let (meshes, images) = (made.meshes.clone(), made.images.clone());
        for mesh in meshes {
            self.meshes.remove(mesh);
        }
        for (image, _) in images {
            self.drop_texture(image, false);
        }
        let files = self.loaded.models.iter();
        let files: Vec<PathBuf> = files
            .filter(|&(_, &loaded)| loaded == model)
            .map(|(file, _)| file.clone())
            .collect();
        for file in files {
            self.loaded.models.remove(&file);
            #[cfg(feature = "dev")]
            if let Some(hot_reload) = &mut self.hot_reload {
                hot_reload.unwatch(HotAsset::Model(file));
            }
        }
        let note = format!(
            "removed model {} {}",
            model.index(),
            self.assets.model_path(model)
        );
        self.add_assets(|assets| {
            assets.remove_model(model);
            assets.note(note);
        });
    }

    /// Why `texture` isn't removed, as [`remove_texture`](Self::remove_texture) says, if it isn't:
    /// `None` for one a layer added or text named.
    fn unremovable(&self, texture: Texture) -> Option<&'static str> {
        if self.assets.name(texture).is_none() {
            return Some(
                "is the engine's white texture, a font's field or another context's texture, none \
                 of which is removed",
            );
        }
        let mut images = self.loaded.made.iter().flat_map(|made| &made.images);
        let of_model = images.any(|&(image, _)| image == texture);
        of_model.then_some("is a model's image, which goes with its model, through `remove_model`")
    }

    /// Why the mesh of index `index` isn't removed, as [`remove_mesh`](Self::remove_mesh) says, if
    /// it isn't.
    fn unremovable_mesh(&self, index: u32) -> Option<&'static str> {
        if index == 0 {
            return Some(
                "is the engine's quad, which sprites, circles, lines and text are drawn on",
            );
        }
        let mut meshes = self.loaded.made.iter().flat_map(|made| &made.meshes);
        let of_model = meshes.any(|mesh| mesh.index() == index);
        of_model.then_some("is a model's, which goes with its model, through `remove_model`")
    }

    /// Removes `texture`, unless it's removed already, as [`remove_texture`](Self::remove_texture)
    /// says, noting it among what the layers added if `noted`, by its index and its path, as a
    /// texture a layer removes is; a model's image goes with its model's note.
    fn drop_texture(&mut self, texture: Texture, noted: bool) {
        if self.assets.removed(texture) {
            return;
        }
        self.textures.remove(texture);
        self.loader.forget(texture);
        (self.loaded.textures).retain(|_, &mut (loaded, _)| loaded != texture);
        #[cfg(feature = "dev")]
        {
            self.loaded.sheets.retain(|&sheet| sheet != texture);
            if let Some(hot_reload) = &mut self.hot_reload {
                hot_reload.unwatch(HotAsset::Texture(texture));
            }
        }
        let path = self
            .assets
            .path(texture)
            .expect("a texture removed is named");
        let note = noted.then(|| format!("removed texture {} {path}", texture.index()));
        self.add_assets(|assets| {
            assets.remove_texture(texture);
            if let Some(note) = note {
                assets.note(note);
            }
        });
    }

    /// Removes the mesh of index `index`, unless it's removed already, as
    /// [`remove_mesh`](Self::remove_mesh) says, noting it among what the layers added, by its
    /// index.
    fn drop_mesh(&mut self, index: u32) {
        if self.meshes.remove_index(index) {
            self.add_assets(|assets| assets.note(format!("removed mesh {index}")));
        }
    }

    /// Loads the TrueType or OpenType font at `path`, a `.ttf` or `.otf` file, for
    /// [`draw_text`](Self::draw_text) to draw text in from then on, through the handle, baking its
    /// field as it loads: a texture of data, a multi-channel signed distance field of each glyph of
    /// the characters Windows-1252 prints that the font has, and of its `.notdef`, which draws in
    /// place of the characters the font or the field lacks but whitespace and control characters,
    /// as ARCHITECTURE.md's Rendering says. The outlines are its default instance's, a variable
    /// font's at its axes' defaults, unhinted, since the field draws them at any size. `name` names
    /// the font in errors and its field in GPU captures, as `<name> field`; frames stream the field
    /// to the GPU, and its text draws nothing until it's there, as its glyphs bake and the field
    /// grows. Its add logs `asset.font_loaded`, with how long it took. A file added again, by
    /// whatever form of its path, gives the font it gave first, without baking it again, as does an
    /// asset's file renamed or moved since, by the ID that names it. Scenes, snapshots, recordings
    /// and the tool API name it by its asset ID, where its file has one, and otherwise by its path,
    /// as a texture is. In a development run that serves the tool API, it bakes again as its file
    /// changes, off the main thread.
    ///
    /// Fails with `engine.bad_font` if the file can't be read, isn't a regular file, is more than
    /// 256 MiB, isn't a TrueType or OpenType font with outlines, as a font of bitmaps alone isn't,
    /// or has glyphs that don't fit a field of 8,192 texels square, alone or together; and with
    /// `renderer.too_many_textures` if the renderer hasn't room for its field.
    pub fn add_font(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Font> {
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let font = match self.loaded_font(&named, path, file.as_deref()) {
            Some(font) => {
                if self.assets.font(&named).is_none() {
                    self.add_assets(|assets| assets.alias_font(font, named.clone()));
                }
                font
            }
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let font::Bake {
                    size,
                    pixels,
                    baked,
                } = font::load(name, path, &self.files)?;
                let field =
                    (self.textures).add(&format!("{name} field"), size, pixels, Texels::Data);
                let field = field?;
                let id = self.texture_name(&named, path, file.as_deref(), None);
                let font = self.fonts.push(field, Some(baked));
                #[cfg(feature = "dev")]
                self.watch_font(font, name, path, read_at, &id);
                self.add_assets(|assets| {
                    assets.add_unnamed(field);
                    assets.add_font(font, named.clone(), id);
                });
                if let Some(file) = file {
                    self.loaded.fonts.insert(file, font);
                }
                info!(
                    code = "asset.font_loaded",
                    font = &*named,
                    ?size,
                    ms = perf::millis(started.elapsed()),
                    "Font loaded"
                );
                font
            }
        };
        self.added("font", &named);
        Ok(font)
    }

    /// Loads the prefab at `path`, a `.prefab` file holding a scene of one subtree, as
    /// [`World::save_scene`] saves one, for [`Prefab::spawn`] and the [`PrefabRoot`]s that name it
    /// to spawn instances of from then on, through the handle. `name` names it in errors. A file
    /// added again, by whatever form of its path, gives the prefab it gave first, as does an
    /// asset's file renamed or moved since, by the ID that names it. It's named by its asset ID,
    /// where its file has one, or else by its path, as a texture is. Its text is read as it loads,
    /// and the textures, models, fonts and prefabs it names that none loaded has are loaded after
    /// it, as a scene's are; its entities' components are made as its instances spawn, of the types
    /// the world has registered, as a scene's are, which refuses a text that doesn't load. Its load
    /// logs `asset.prefab_loaded`, with how long it took, the loads of what it names among it. In a
    /// development run that serves the tool API, it's read again as its file changes, and every
    /// instance takes what changed, as [`PrefabRoot`] says, unless the run is recorded, in a
    /// lockstep session or follows another.
    ///
    /// Fails with `engine.bad_prefab` if the file can't be read, isn't a regular file, is more than
    /// 16 MiB, isn't UTF-8, or isn't a scene's RON as far as its syntax and its entities' IDs go.
    pub fn add_prefab(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Prefab> {
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let prefab = match self.loaded_prefab(&named, path, file.as_deref()) {
            Some(prefab) => {
                if self.assets.prefab(&named).is_none() {
                    self.add_assets(|assets| assets.alias_prefab(prefab, named.clone()));
                }
                prefab
            }
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let text: Arc<str> = prefab::read(name, path, &self.files)?.into();
                let id = self.texture_name(&named, path, file.as_deref(), None);
                let held = Some(Arc::clone(&text));
                let prefab =
                    self.add_assets(|assets| assets.add_prefab(named.clone(), id.clone(), held));
                #[cfg(feature = "dev")]
                self.watch_prefab(prefab, name, path, read_at, &id);
                if let Some(file) = file {
                    self.loaded.prefabs.insert(file, prefab);
                }
                self.added("prefab", &named);
                // What its text names, as a scene's load loads it, after it, so that a restored
                // run loads them in the same order; a text that doesn't load is refused as its
                // instances spawn.
                let _ = self.load_scene_assets(&text);
                info!(
                    code = "asset.prefab_loaded",
                    prefab = &*named,
                    ms = perf::millis(started.elapsed()),
                    "Prefab loaded"
                );
                return Ok(prefab);
            }
        };
        self.added("prefab", &named);
        Ok(prefab)
    }

    /// Loads the UI at `path`, a `.ui` file holding a tree of named nodes, as ARCHITECTURE.md's
    /// Events and layers describes, or the UI file under the asset root whose asset ID `path` is,
    /// as text names one; and adds a UI of it, which the layer drives through the handle, as
    /// [`GameUi`] says, with its own focus and what code sets of its nodes, apart from any other UI
    /// of the file, which is read once, by whatever form of its path. `name` names it in errors,
    /// the log and the tool API's `ui`. Its file is named by its asset ID, where it has one, or
    /// else by its path, as a texture's is. The fonts and textures its nodes name, by their IDs or
    /// paths, are loaded with it, as a scene's are, and its file's load logs `asset.ui_loaded`, with
    /// how long it took, theirs among it. In a development run that serves the tool API,
    /// its file is read again as it changes, every UI of it taking its nodes, each node keeping
    /// what code set of it by its name, and the focus staying on the button of its name, logging
    /// `UI reloaded`; a change that doesn't read warns with its code, the UI staying as it was.
    ///
    /// Fails with `engine.bad_ui` if the file can't be read, isn't a regular file, is more than
    /// 16 MiB or isn't UTF-8; with `ui.syntax`, `ui.bad_name`, `ui.duplicate_name`,
    /// `ui.bad_field`, `ui.unnamed_button`, `ui.bad_value` or `ui.no_node` for a file that isn't a
    /// UI's, as ARCHITECTURE.md's Project state says, saying the line and column where it's
    /// refused;
    /// as finding an ID's file fails, as with `asset.unknown_id`; with `asset.no_font` or
    /// `asset.no_texture` for a font or a texture it names that doesn't load, saying why; and with
    /// `renderer.too_many_textures` if the renderer hasn't room for the texture rounded corners
    /// are cut from, which the first UI adds.
    pub fn add_ui(&mut self, name: &str, path: impl AsRef<Path>) -> Result<GameUi> {
        let path = path.as_ref();
        let path = match path.to_str().filter(|given| assets::is_id(given)) {
            Some(id) => {
                // A run that follows another loads a UI of its own, which names no asset of the
                // world's, by its ID as its own database has it.
                let at = self.asset_path(id, "UI", true)?;
                self.named_file(id, &at)?
            }
            None => path.to_owned(),
        };
        if self.uis.disc.is_none() {
            let (size, pixels) = game_ui::disc();
            let disc = self
                .textures
                .add("UI corners", size, pixels, Mipmaps::AlphaWeighted)?;
            self.add_assets(|assets| assets.add_unnamed(disc));
            self.uis.disc = Some(disc);
        }
        let file = self.files.file_of(&path);
        let named = assets::name_of(self.root.as_deref(), &path);
        let asset = self.texture_name(&named, &path, file.as_deref(), None);
        let found = match self.uis.file_of(&asset, file.as_deref()) {
            Some(found) => found,
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let mut document = game_ui::read(name, &path, &self.files)?;
                self.load_ui_assets(&mut document)?;
                info!(
                    code = "asset.ui_loaded",
                    ui = &*named,
                    ms = perf::millis(started.elapsed()),
                    "UI loaded"
                );
                let found = self.uis.add_file(asset.clone(), named, document, file);
                #[cfg(feature = "dev")]
                self.watch_ui(found, name, &path, read_at, &asset);
                found
            }
        };
        Ok(self.uis.add(found, name))
    }

    /// Loads each font and texture the nodes of `document`, a UI's, name that none loaded has, as
    /// [`load_named`](Self::load_named) loads what a scene names, and gives each node what it
    /// names, and an image its texture's size, where a layer's add or a scene loaded it from its
    /// file.
    ///
    /// Fails with `asset.no_font` or `asset.no_texture` for one that doesn't load, saying why.
    fn load_ui_assets(&mut self, document: &mut game_ui::Document) -> Result<()> {
        let mut names = Vec::new();
        for node in &document.nodes {
            names.extend(node.font.iter().map(|font| Asset::Font(font.name.clone())));
            let textures = node.looks.iter().filter_map(|look| look.texture.as_ref());
            names.extend(textures.map(|texture| Asset::Texture(texture.name.clone())));
        }
        if names.is_empty() {
            return Ok(());
        }
        self.load_named(names, self.follows);
        for node in &mut document.nodes {
            if let Some(font) = &mut node.font {
                let found = self.assets.font(&font.name).ok_or_else(|| {
                    Error::new("asset.no_font", self.assets.not_found("font", &font.name))
                })?;
                font.handle = Some(found);
            }
            for texture in node
                .looks
                .iter_mut()
                .filter_map(|look| look.texture.as_mut())
            {
                let found = self.assets.live_texture(&texture.name).ok_or_else(|| {
                    Error::new(
                        "asset.no_texture",
                        self.assets.not_found("texture", &texture.name),
                    )
                })?;
                texture.handle = Some(found);
            }
            let shown = node.looks[0]
                .texture
                .as_ref()
                .and_then(|texture| texture.handle);
            let loaded = self.loaded.textures.values();
            let size = loaded.copied().find(|&(loaded, _)| Some(loaded) == shown);
            node.natural = size.map(|(_, size)| Vec2::new(size[0] as f32, size[1] as f32));
        }
        Ok(())
    }

    /// Loads the audio file at `path`, WAV, Ogg Vorbis, FLAC or MP3, as a sound for ticks to queue
    /// with [`World::play_sound`], and layers to play with [`play_sound`](Self::play_sound), from
    /// then on, through the handle, decoding it whole into memory, as a game's short sounds are.
    /// `name` names it in errors and the log, and its add logs `asset.sound_loaded`, with how long
    /// it took, at the debug level. A file added again, by whatever form of its path, gives the
    /// sound it gave first, without decoding it again. In a development run that serves
    /// the tool API, the sound decodes again as its file changes, and plays as its file holds from
    /// its next play on; a change that doesn't decode warns, and the sound plays as it did.
    ///
    /// Windowed and embedded runs play sound, as ARCHITECTURE.md's Audio says: the first sound
    /// loaded opens the system's default output device, so that a game without sounds opens none,
    /// and a run that can't open one plays none, warning `audio.no_output`. Headless runs, replays
    /// and tests play none. What plays follows how the run plays, as the tool API's `pause` and
    /// `stop` leave it: paused, it holds where it is, and stopped, it stops.
    ///
    /// Fails with `engine.bad_sound` if the file can't be read, isn't a regular file, is more than
    /// 16 MiB, doesn't decode, or decodes to more than 2²⁴ frames, some 6 minutes at 48 kHz, which
    /// it's refused as it passes: longer audio is music, which [`add_music`](Self::add_music)
    /// streams.
    pub fn add_sound(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Sound> {
        let path = path.as_ref();
        let file = self.files.file_of(path);
        if let Some(&sound) = file.as_ref().and_then(|file| self.loaded.sounds.get(file)) {
            return Ok(sound);
        }
        let started = Instant::now();
        #[cfg(feature = "dev")]
        let read_at = SystemTime::now();
        let data = audio::decode(name, path, &self.files)?;
        // Before the output opens for the first sound, logging `audio.opened`.
        let ms = perf::millis(started.elapsed());
        self.audio.open();
        let sound = self.audio.add(name, data);
        #[cfg(feature = "dev")]
        self.watch_sound(sound, name, path, read_at);
        if let Some(file) = file {
            self.loaded.sounds.insert(file, sound);
        }
        // At the debug level, as a game may add many.
        debug!(
            code = "asset.sound_loaded",
            sound = &*assets::name_of(self.root.as_deref(), path),
            ms,
            "Sound loaded"
        );
        Ok(sound)
    }

    /// Plays `sound` from `place` now, in a run that plays sound, as
    /// [`add_sound`](Self::add_sound) says: as a layer's UI plays its sounds as it updates, panned
    /// and faded as the game's view hears it there, as [`Place`] says, following an entity of the
    /// world it's on. Ticks queue theirs in the world, with [`World::play_sound`]. Past the 128
    /// sounds that play at once, it's left out, warning `audio.full`; and while the run is paused
    /// or stopped, as the tool API's `pause` and `stop` leave it, it isn't played.
    ///
    /// # Panics
    ///
    /// If `sound` is another context's.
    pub fn play_sound(&mut self, sound: Sound, place: Place) {
        self.audio.play(sound, place, &self.world);
    }

    /// Loads the audio file at `path`, WAV, Ogg Vorbis, FLAC or MP3, as music for layers to play
    /// with [`play_music`](Self::play_music) from then on, through the handle: read whole into
    /// memory, as compressed as the file is, and decoded as it plays, on a thread of kira's, so
    /// that a long piece takes little memory. `name` names it in errors and the log, and its add
    /// logs `asset.music_loaded`, with how long it took. A file added again, by whatever form of
    /// its path, gives the music it gave first, without reading it again. In a development run
    /// that serves the tool API, it's read again as its file changes, a play of it crossfading to
    /// what the file holds from where it was; a change that doesn't read warns, and it plays as it
    /// did. It plays as sounds do, as [`add_sound`](Self::add_sound) says, its first loaded opening
    /// the output as a sound would.
    ///
    /// Fails with `engine.bad_music` if the file can't be read, isn't a regular file, is more than
    /// 64 MiB, or isn't WAV, Ogg Vorbis, FLAC or MP3 of one or two channels that says its rate and
    /// length and holds a frame, its first packet decoding. What's past its first packet decodes
    /// only as it plays: a play that fails there warns `audio.music_failed`, and stops.
    pub fn add_music(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Music> {
        let path = path.as_ref();
        let file = self.files.file_of(path);
        if let Some(&music) = file.as_ref().and_then(|file| self.loaded.music.get(file)) {
            return Ok(music);
        }
        let started = Instant::now();
        #[cfg(feature = "dev")]
        let read_at = SystemTime::now();
        let encoded = audio::read_music(name, path, &self.files)?;
        // Before the output opens, as for a sound.
        let ms = perf::millis(started.elapsed());
        self.audio.open();
        let music = self.audio.add_music(name, encoded);
        #[cfg(feature = "dev")]
        self.watch_music(music, name, path, read_at);
        if let Some(file) = file {
            self.loaded.music.insert(file, music);
        }
        info!(
            code = "asset.music_loaded",
            music = &*assets::name_of(self.root.as_deref(), path),
            ms,
            "Music loaded"
        );
        Ok(music)
    }

    /// Plays `music`, looping, in place of the music playing, or `None` for none, in a run that
    /// plays sound, as [`add_music`](Self::add_music) says: the music playing fades out over
    /// `fade` as `music` fades in from its start, on the music's bus, or at once for a fade of
    /// none, which takes kira's 10 ms against a click. Playing the music playing goes on as it
    /// was, so that a layer can say each frame which plays. Ticks don't play music: a layer that
    /// reads the world as it updates plays what the world calls for. While the run is stopped, as
    /// `--stopped` and the tool API's `stop` leave it, it doesn't play; as the run plays, the
    /// music the layers last had play starts from its start, fading in over the fade it was given.
    ///
    /// # Panics
    ///
    /// If `music` is another context's.
    pub fn play_music(&mut self, music: Option<Music>, fade: Duration) {
        self.audio.play_music(music, fade);
    }

    /// The music layers last had play, if any, as [`play_music`](Self::play_music) says: in a run
    /// that plays no sound too, though none's heard.
    pub fn music_playing(&self) -> Option<Music> {
        self.audio.music_playing()
    }

    /// Has `bus`'s volume go to `volume` over `fade`, or at once for a fade of none, as
    /// [`play_music`](Self::play_music)'s fades do: 1 as loud as what plays through it is, 0.5
    /// half as loud, about -6 dB, and 0 or less silent. Each bus starts at 1, and keeps its volume
    /// for the run, in a run that plays no sound too, though none's heard; one set to the volume
    /// it's going to already goes on as it was, so that a layer can say each frame how loud each
    /// is, as a menu that ducks the music under it does.
    ///
    /// # Panics
    ///
    /// If `volume` isn't a finite number.
    pub fn set_volume(&mut self, bus: Bus, volume: f32, fade: Duration) {
        self.audio.set_volume(bus, volume, fade);
    }

    /// `bus`'s volume, as layers last set it with [`set_volume`](Self::set_volume).
    pub fn volume(&self, bus: Bus) -> f32 {
        self.audio.volume(bus)
    }

    /// Plays the sounds the world's ticks queued, emptying its queue, as [`World::play_sound`]
    /// says: once each tick has run.
    pub(crate) fn play_queued(&mut self) {
        self.audio.play_queued(&mut self.world);
    }

    /// Plays the sounds the world's ticks queued, as [`play_queued`](Self::play_queued) does, but
    /// those of `played`, which the tick's runs before played, as a tick rollback runs again has,
    /// adding those it plays to `played`.
    pub(crate) fn play_queued_again(&mut self, played: &mut Vec<Sound>) {
        self.audio.play_queued_again(&mut self.world, played);
    }

    /// Has what plays follow how the run plays, as `stopped` and `paused` say it now does, as
    /// [`Playback`](audio::Playback) says.
    #[cfg(feature = "dev")]
    pub(crate) fn follow_playback(&mut self, stopped: bool, paused: bool) {
        let playback = if stopped {
            audio::Playback::Stopped
        } else if paused {
            audio::Playback::Paused
        } else {
            audio::Playback::Playing
        };
        self.audio.follow(playback);
    }

    /// Records that a layer added an asset of `kind`, as `what` says it: its name, and the types
    /// that a mesh's, a shader's or a model's draws hold the bytes of, as [`Assets`] keeps it.
    fn added(&mut self, kind: &str, what: &str) {
        let (meshes, shaders) = (self.meshes.count(), self.shaders.count());
        let added = format!("{kind} {meshes} {shaders} {} {what}", self.textures.count());
        self.add_assets(|assets| assets.note(added));
    }

    /// Has `path` name `texture`, which another path to its file added before, as [`Assets::alias`]
    /// does, unless it names a texture already that isn't removed.
    fn alias(&mut self, texture: Texture, path: &Arc<str>) {
        if self.assets.live_texture(path).is_none() {
            self.add_assets(|assets| assets.alias(texture, path.clone()));
        }
    }

    /// Loads, as [`add_texture`](Self::add_texture), [`add_model`](Self::add_model),
    /// [`add_font`](Self::add_font) and [`add_prefab`](Self::add_prefab) load a file, each texture,
    /// model, font and prefab that `names` names,
    /// as text names one, that none loaded has, or none but one removed, which loads anew, as
    /// [`remove_texture`](Self::remove_texture) says: where a scene, the tool API, or a recorded or
    /// a lockstep peer's command or edit names it, before it's read, as ARCHITECTURE.md's Project
    /// state says; a model's image with its model. A name finds its file as
    /// [`asset_path`](Self::asset_path) says, inside the asset root. In a run that follows another,
    /// the asset takes the name its text gave it, that run's, and its file is found by the names it
    /// was given first.
    ///
    /// One that can't be loaded stands in, if `stand_in`, as for a run that follows another, so
    /// that its texts read as that run's did: a texture of magenta and black squares, a model of no
    /// nodes, or a font that draws in the engine's, named as the name, warning `asset.stand_in`
    /// with why. Otherwise it stays unloaded, and the refusal of what names it says why, as
    /// [`Assets::not_found`] does.
    pub(crate) fn load_named<S: AsRef<str>>(
        &mut self,
        names: impl IntoIterator<Item = Asset<S>>,
        stand_in: bool,
    ) {
        let names: Vec<Asset<S>> = names.into_iter().collect();
        let mut tried: Vec<Asset<&str>> = Vec::new();
        let mut failed = Vec::new();
        for asset in &names {
            let named = asset.as_deref();
            if self.assets.has(&named) || tried.contains(&named) {
                continue;
            }
            tried.push(named);
            let (Asset::Texture(name)
            | Asset::Model(name)
            | Asset::Font(name)
            | Asset::Prefab(name)) = named;
            let (path, loaded) = match named {
                Asset::Texture(_) => self.load_texture_named(name, stand_in),
                Asset::Model(_) => self.load_model_named(name, stand_in),
                Asset::Font(_) => self.load_font_named(name, stand_in),
                Asset::Prefab(_) => self.load_prefab_named(name, stand_in),
            };
            match (loaded, named) {
                (Ok(()), _) => {}
                (Err(error), Asset::Texture(_)) if stand_in => {
                    self.stand_in(name, path.as_deref(), &error);
                }
                (Err(error), Asset::Model(_)) if stand_in => {
                    self.stand_in_model(name, path.as_deref(), &error);
                }
                (Err(error), Asset::Font(_)) if stand_in => {
                    self.stand_in_font(name, path.as_deref(), &error);
                }
                (Err(error), Asset::Prefab(_)) if stand_in => {
                    self.stand_in_prefab(name, path.as_deref(), &error);
                }
                (Err(error), _) => failed.push((name, error)),
            }
        }
        if !tried.is_empty() {
            self.add_assets(|assets| assets.failed(&failed));
        }
    }

    /// Loads the texture that text names `name`, as [`load_named`](Self::load_named) does: a
    /// model's image by loading its model, if it isn't. Gives the path from the asset root its name
    /// led to, if it led to one.
    ///
    /// Fails as [`asset_path`](Self::asset_path) and [`add_texture`](Self::add_texture) do; for a
    /// model's image, as [`load_model_named`](Self::load_model_named) does, and with
    /// `asset.model_image` for an image the model hasn't.
    fn load_texture_named(&mut self, name: &str, stand_in: bool) -> (Option<Arc<str>>, Result<()>) {
        if let Some((model, image)) = name.rsplit_once('#')
            && !image.is_empty()
            && image.bytes().all(|b| b.is_ascii_digit())
        {
            let loaded = match self.assets.live_model(model) {
                Some(_) => Ok(()),
                None => self.load_model_named(model, stand_in).1,
            };
            // By the model's own name, as another path to its file may name it.
            let found = loaded.and_then(|()| {
                let loaded = self.assets.live_model(model).expect("loaded");
                let named = format!("{}#{image}", self.assets.model_name(loaded));
                match self.assets.live_texture(&named) {
                    Some(texture) => {
                        self.alias(texture, &name.into());
                        Ok(())
                    }
                    None => {
                        let message = format!(
                            "`{name}` names image {image} of model `{model}`, which hasn't it"
                        );
                        Err(Error::new("asset.model_image", message))
                    }
                }
            });
            return (None, found);
        }
        match self.asset_path(name, "texture", stand_in) {
            Ok(path) => {
                let loaded = self.load_path(name, &path);
                (Some(path), loaded)
            }
            Err(error) => (None, Err(error)),
        }
    }

    /// Loads the model that text names `name`, as [`load_named`](Self::load_named) does, a file
    /// loaded already taking the names alone, as [`load_path`](Self::load_path) says. Gives the
    /// path from the asset root its name led to, if it led to one.
    ///
    /// Fails as [`asset_path`](Self::asset_path) and [`add_model`](Self::add_model) do, and with
    /// `asset.outside_root` for a file outside the root, as through `..` or a link.
    fn load_model_named(&mut self, name: &str, stand_in: bool) -> (Option<Arc<str>>, Result<()>) {
        let path = match self.asset_path(name, "model", stand_in) {
            Ok(path) => path,
            Err(error) => return (None, Err(error)),
        };
        let loaded = self.named_file(name, &path).and_then(|file| {
            let resolved = self.files.file_of(&file);
            let model = match self.loaded_model(&path, &file, resolved.as_deref()) {
                Some(model) => model,
                None => self.add_model(&path, &file)?,
            };
            for named in [&*path, name] {
                if self.assets.live_model(named).is_none() {
                    self.add_assets(|assets| assets.alias_model(model, named.into()));
                }
            }
            Ok(())
        });
        (Some(path), loaded)
    }

    /// Loads the font that text names `name`, as [`load_named`](Self::load_named) does, a file
    /// loaded already taking the names alone, as [`load_path`](Self::load_path) says. Gives the
    /// path from the asset root its name led to, if it led to one.
    ///
    /// Fails as [`asset_path`](Self::asset_path) and [`add_font`](Self::add_font) do, and with
    /// `asset.outside_root` for a file outside the root, as through `..` or a link.
    fn load_font_named(&mut self, name: &str, stand_in: bool) -> (Option<Arc<str>>, Result<()>) {
        let path = match self.asset_path(name, "font", stand_in) {
            Ok(path) => path,
            Err(error) => return (None, Err(error)),
        };
        let loaded = self.named_file(name, &path).and_then(|file| {
            let resolved = self.files.file_of(&file);
            let font = match self.loaded_font(&path, &file, resolved.as_deref()) {
                Some(font) => font,
                None => self.add_font(&path, &file)?,
            };
            for named in [&*path, name] {
                if self.assets.font(named).is_none() {
                    self.add_assets(|assets| assets.alias_font(font, named.into()));
                }
            }
            Ok(())
        });
        (Some(path), loaded)
    }

    /// Loads the prefab that text names `name`, as [`load_named`](Self::load_named) does, a file
    /// loaded already taking the names alone, as [`load_path`](Self::load_path) says. Gives the
    /// path from the asset root its name led to, if it led to one.
    ///
    /// Fails as [`asset_path`](Self::asset_path) and [`add_prefab`](Self::add_prefab) do, and with
    /// `asset.outside_root` for a file outside the root, as through `..` or a link.
    fn load_prefab_named(&mut self, name: &str, stand_in: bool) -> (Option<Arc<str>>, Result<()>) {
        let path = match self.asset_path(name, "prefab", stand_in) {
            Ok(path) => path,
            Err(error) => return (None, Err(error)),
        };
        let loaded = self.named_file(name, &path).and_then(|file| {
            let resolved = self.files.file_of(&file);
            let prefab = match self.loaded_prefab(&path, &file, resolved.as_deref()) {
                Some(prefab) => prefab,
                None => self.add_prefab(&path, &file)?,
            };
            for named in [&*path, name] {
                if self.assets.prefab(named).is_none() {
                    self.add_assets(|assets| assets.alias_prefab(prefab, named.into()));
                }
            }
            Ok(())
        });
        (Some(path), loaded)
    }

    /// Takes again what the run a snapshot was taken from noted past the notes this run's layers
    /// made alike as they loaded, of `lines`, the snapshot's `asset` lines, in order, as
    /// [`Snapshot::added`] gives them: as a run restored from it, or a lockstep joiner given its
    /// host's world, does before it reads the world, so that its values find every asset they
    /// name, and what's opaque in it comes back where the run's assets line up with the
    /// snapshot's. It loads, as [`load_named`](Self::load_named) does with `stand_in`, each
    /// texture, model, font and prefab a line gives the path of that none loaded has, in the order
    /// that run
    /// loaded them, so that a texture's index, which what's opaque holds, is what it was where the
    /// layers added the same before; a path outside the root, as only a layer gives one, stands
    /// in, as one text names does. And it removes again what that run removed, a texture, a model
    /// or a mesh by the index its line gives, while this run's notes are the snapshot's up to that
    /// line, so that the index is the asset's that run removed; a texture or a model only where the
    /// one at its index is loaded from the path the line gives, and isn't removed already.
    pub(crate) fn load_restored(&mut self, lines: &[String]) {
        let ours = self.assets.added();
        let common = (lines.iter().zip(ours))
            .take_while(|&(theirs, ours)| **theirs == **ours)
            .count();
        // Whether this run's notes are the snapshot's lines up to the one taken.
        let mut lined_up = ours.len() == common;
        for (at, line) in lines.iter().enumerate().skip(common) {
            match snapshot::again(line) {
                Some(Again::Load(asset)) => self.load_named([asset], true),
                // The index a removal gives may be another asset's in a run whose assets part
                // from the snapshot's before it, as one whose newer layers add another first.
                Some(_) if !lined_up => debug!(
                    line = &**line,
                    "Not removed again, as the run's assets part from the snapshot's before it"
                ),
                Some(Again::RemoveTexture(index, path)) => {
                    let texture = self.assets.live_texture_at(index, path);
                    let texture = texture.filter(|&texture| self.unremovable(texture).is_none());
                    if let Some(texture) = texture {
                        self.drop_texture(texture, true);
                    }
                }
                Some(Again::RemoveModel(index, path)) => {
                    if let Some(model) = self.assets.live_model_at(index, path) {
                        self.remove_model(model);
                    }
                }
                Some(Again::RemoveMesh(index)) => {
                    let known = index < self.meshes.count();
                    if known && self.unremovable_mesh(index).is_none() {
                        self.drop_mesh(index);
                    }
                }
                None => {}
            }
            let added = self.assets.added();
            lined_up &= added.len() == at + 1 && *added[at] == **line;
        }
    }

    /// Loads each texture, model, font and prefab scene `text` names that none loaded has, and
    /// those the prefabs it names name, in turn, as [`load_named`](Self::load_named) does, standing
    /// in for what can't be in a run that follows another; and returns the names of those still
    /// missing.
    ///
    /// Fails, loading nothing, as [`World::load_scene`] does but for those.
    pub(crate) fn load_scene_assets(&mut self, text: &str) -> Result<Vec<String>> {
        self.load_scene_assets_as(text, self.follows)
    }

    /// As [`load_scene_assets`](Self::load_scene_assets), standing in for what can't be loaded if
    /// `stand_in`, whatever run this is.
    fn load_scene_assets_as(&mut self, text: &str, stand_in: bool) -> Result<Vec<String>> {
        let mut missing = Vec::new();
        // Again while a prefab loads, whose text may name more: each time one more at least.
        loop {
            let names = self.world.scene_assets(text)?;
            self.load_named(names.iter().map(Asset::as_deref), stand_in);
            let prefab = |asset: &Asset<String>| matches!(asset, Asset::Prefab(_));
            let more = names
                .iter()
                .any(|asset| prefab(asset) && self.assets.has(asset));
            for asset in names.into_iter().filter(|asset| !self.assets.has(asset)) {
                let name = asset.name().to_owned();
                if !missing.contains(&name) {
                    missing.push(name);
                }
            }
            if !more {
                return Ok(missing);
            }
        }
    }

    /// Loads each texture, model, font and prefab `text`, a value as a recording writes it, names
    /// that
    /// none loaded has, standing in for what can't be, as [`load_named`](Self::load_named) does: a
    /// value that a recording or a lockstep peer gives, which names assets as the run that wrote it
    /// did.
    pub(crate) fn load_followed(&mut self, text: &Text) {
        let mut names = Vec::new();
        text.assets(&mut names);
        if !names.is_empty() {
            self.load_named(names, true);
        }
    }

    /// The path from the asset root of the file whose texture, model or font, as `kind` says, text
    /// names `name`, as [`load_named`](Self::load_named) loads it: in a run that follows another,
    /// the path given that name, as [`name_as`](Self::name_as) gives them; in a run that reads its
    /// game's package, the asset's whose ID it is there; in a development build, the asset's whose
    /// ID it is, as the asset database holds it; and else `name` itself, as a path. A run that
    /// follows another and isn't to stand in for what it can't load, as a lockstep joiner asked by
    /// the tool API, finds only what it was given, so that it names the asset as its host would.
    ///
    /// Fails with `asset.not_given` for what such a run wasn't given; `asset.unknown_id` for an ID
    /// no asset has this run, naming the file the project's fetch list gives it, if it gives it
    /// one, as [`fetched`](Self::fetched) finds it, or any ID in a build without `dev` that reads
    /// no package, as it reads no ID file; and `asset.outside_root` for an absolute path.
    fn asset_path(&self, name: &str, kind: &str, stand_in: bool) -> Result<Arc<str>> {
        #[cfg(feature = "dev")]
        if let Some(given) = &self.given {
            if let Some((path, _)) = given.iter().find(|(_, given)| ***given == *name) {
                return Ok(path.clone());
            }
            if self.follows && !stand_in && !given.contains_key(name) {
                let message = format!(
                    "`{name}` isn't a name or a path that the lockstep host has given a {kind}, \
                     and a joiner loads only those, so that both peers name it alike: name it \
                     through the host"
                );
                return Err(Error::new("asset.not_given", message));
            }
        }
        #[cfg(not(feature = "dev"))]
        let _ = stand_in;
        if assets::is_id(name) {
            if let Some(package) = self.files.packaged() {
                let id = u64::from_str_radix(name, 16).ok();
                if let Some(path) = id.and_then(|id| package.path_of(id)) {
                    return Ok(path.into());
                }
                let message = format!("no asset in the game's package has the ID {name}");
                return Err(Error::new("asset.unknown_id", message));
            }
            #[cfg(feature = "dev")]
            {
                let id = name.parse::<AssetId>().ok();
                if let Some(path) = id.and_then(|id| self.assets.catalog().path(id)) {
                    return Ok(path.into());
                }
                let mut message =
                    format!("no asset under the asset root has the ID {name} this run");
                if let Some(path) = id.and_then(|id| self.fetched(id)) {
                    message += &format!(
                        ", but the project's fetch list gives it to `{path}`: `cargo gg fetch`, \
                         in the project's directory, fetches it with its ID"
                    );
                }
                return Err(Error::new("asset.unknown_id", message));
            }
            #[cfg(not(feature = "dev"))]
            {
                let message = format!(
                    "{name} is an asset ID, but a build without `dev` reads no ID file, so it finds \
                     a {kind} by its path alone, but in its game's package"
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
                 {kind} is named by its ID or by its path from the asset root"
            );
            return Err(Error::new("asset.outside_root", message));
        }
        Ok(name.into())
    }

    /// The path, from the project's directory, of the file the project's fetch list, `fetch.ron`,
    /// gives asset ID `id`, as gg-api's `Fetch` reads it, if it gives it one: a file that `gg
    /// fetch` fetches, which the repository doesn't hold, and so a checkout hasn't until it's
    /// fetched. `None` for a context that isn't a project's game, or one without a fetch list, or
    /// whose list doesn't read. It reads the list each time, as only an error asks.
    #[cfg(feature = "dev")]
    fn fetched(&self, id: AssetId) -> Option<String> {
        let list = gg_api::Fetch::read(self.project_dir.as_deref()?).ok()?;
        let mut files = list.files();
        files.find(|file| file.id == Some(id)).map(|file| file.path)
    }

    /// Loads the texture at `path` from the asset root, which text names `name`, as
    /// [`add_texture`](Self::add_texture) loads a file, its name `name` too: a run that follows
    /// another names it so, as that run did. A file loaded already, as by a layer's add, takes the
    /// names alone, unnoted, as [`Assets`] keeps its notes.
    ///
    /// Fails as [`named_file`](Self::named_file) does, and as `add_texture` does.
    fn load_path(&mut self, name: &str, path: &str) -> Result<()> {
        let file = self.named_file(name, path)?;
        let resolved = self.files.file_of(&file);
        let texture = match self.loaded_texture(path, &file, resolved.as_deref()) {
            Some((texture, _)) => texture,
            None => self.add_texture(path, &file)?,
        };
        for named in [path, name] {
            self.alias(texture, &named.into());
        }
        Ok(())
    }

    /// The file at `path` from the asset root, which text names `name`, as a run that follows
    /// another names its asset from then on, as that run did.
    ///
    /// Fails with `asset.outside_root` for a file outside the root, as through `..` or a link.
    fn named_file(&mut self, name: &str, path: &str) -> Result<PathBuf> {
        // Before anything can fail, so that a stand-in is named so too.
        #[cfg(feature = "dev")]
        if let Some(given) = &mut self.given
            && name != path
            && !given.contains_key(path)
        {
            given.insert(path.into(), name.into());
        }
        #[cfg(not(feature = "dev"))]
        let _ = name;
        let file = match &self.root {
            Some(root) => root.join(path),
            None => PathBuf::from(path),
        };
        self.within_root(path, &file)?;
        Ok(file)
    }

    /// Fails with `asset.outside_root` if `file`, which `path` names, resolves outside the asset
    /// root, as through `..` or a link, or, in a run that reads its game's package, as written; one
    /// that doesn't resolve, as one that isn't there, doesn't.
    fn within_root(&self, path: &str, file: &Path) -> Result<()> {
        let within = self.root.as_deref().unwrap_or(Path::new("."));
        let outside = if self.reads_package() {
            self.files.within(file).is_none()
        } else {
            matches!(
                (assets::file_of(file), assets::file_of(within)),
                (Some(resolved), Some(within)) if !resolved.starts_with(&within)
            )
        };
        if outside {
            let message = format!(
                "`{path}` is outside the asset root, {}: an asset is named by its ID or by its path \
                 from the asset root",
                within.display()
            );
            return Err(Error::new("asset.outside_root", message));
        }
        Ok(())
    }

    /// Has a stand-in, a texture of magenta and black squares, take the place of the texture text
    /// names `name`, of the file at `path` from the asset root, if its name led to one, which
    /// couldn't be loaded, as `why` says, warning `asset.stand_in`: named as the file would have
    /// been, or else as `name`. In a run that reloads assets, the file's image takes the stand-in's
    /// place once it loads, as after a checkout brings it, or that of the asset whose ID `name` is,
    /// once one has it.
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
        let (path, named, file): (Arc<str>, Arc<str>, _) = match path {
            Some(path) => {
                let file = self
                    .root
                    .as_deref()
                    .map_or(PathBuf::from(path), |root| root.join(path));
                let named = self.texture_name(path, &file, None, None);
                (path.into(), named, Some(file))
            }
            None => (name.into(), name.into(), None),
        };
        // So that it gives way to its texture once its file comes, or once an asset has its ID; but
        // never to a file outside the asset root.
        #[cfg(feature = "dev")]
        match &file {
            Some(_) if why.code() == "asset.outside_root" => {}
            Some(file) => self.watch_texture(texture, name, file, SystemTime::now(), &named),
            None => self.watch_for(texture, name),
        }
        #[cfg(not(feature = "dev"))]
        let _ = file;
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

    /// Has a stand-in, a model of no nodes, take the place of the model text names `name`, as
    /// [`stand_in`](Self::stand_in) has one take a texture's: so that what names it reads, though
    /// its roots spawn nothing, and a hash of the world parts from its run's where their nodes
    /// were. It never gives way to its file, since a run that follows another takes no nodes the
    /// model's file would give later.
    fn stand_in_model(&mut self, name: &str, path: Option<&str>, why: &Error) {
        let (path, named): (Arc<str>, Arc<str>) = match path {
            Some(path) => {
                let root = self.root.as_deref();
                let file = root.map_or(PathBuf::from(path), |root| root.join(path));
                (path.into(), self.texture_name(path, &file, None, None))
            }
            None => (name.into(), name.into()),
        };
        self.add_assets(|assets| {
            let model = assets.add_model(path.clone(), named, Nodes::default());
            assets.alias_model(model, name.into());
        });
        self.loaded.made.push(Made::default());
        self.added("model", &format!("{path} {}", model_types()));
        warn!(
            code = "asset.stand_in",
            name,
            path = &*path,
            cause = why.code(),
            "Model `{name}` can't be loaded, so a stand-in of no nodes takes its place, as the run \
             follows another, which names it: {why:#}"
        );
    }

    /// Has a stand-in, a font that draws in the engine's, take the place of the font text names
    /// `name`, as [`stand_in`](Self::stand_in) has one take a texture's: its field a texture of a
    /// texel, which its file's takes once it bakes, as after a checkout brings it, or that of the
    /// asset whose ID `name` is, once one has it, in a run that reloads assets.
    fn stand_in_font(&mut self, name: &str, path: Option<&str>, why: &Error) {
        let field = (self.textures).add(
            &format!("{name} field"),
            [1, 1],
            [0; 4].as_slice(),
            Texels::Data,
        );
        let field = match field {
            Ok(field) => field,
            Err(error) => {
                warn!(
                    code = error.code(),
                    name, "Can't stand in for font `{name}`: {error:#}"
                );
                return;
            }
        };
        let font = self.fonts.push(field, None);
        let (path, named, file): (Arc<str>, Arc<str>, _) = match path {
            Some(path) => {
                let root = self.root.as_deref();
                let file = root.map_or(PathBuf::from(path), |root| root.join(path));
                let named = self.texture_name(path, &file, None, None);
                (path.into(), named, Some(file))
            }
            None => (name.into(), name.into(), None),
        };
        #[cfg(feature = "dev")]
        match &file {
            Some(_) if why.code() == "asset.outside_root" => {}
            Some(file) => self.watch_font(font, name, file, SystemTime::now(), &named),
            None => self.watch_font_for(font, name),
        }
        #[cfg(not(feature = "dev"))]
        let _ = file;
        self.add_assets(|assets| {
            assets.add_unnamed(field);
            assets.add_font(font, path.clone(), named);
            assets.alias_font(font, name.into());
        });
        self.added("font", &path);
        warn!(
            code = "asset.stand_in",
            name,
            path = &*path,
            cause = why.code(),
            "Font `{name}` can't be loaded, so text in it draws in the engine's own font, Inter, \
             as the run follows another, which names it: {why:#}"
        );
    }

    /// Has a stand-in, a prefab whose instances are their roots alone, take the place of the prefab
    /// text names `name`, as [`stand_in_model`](Self::stand_in_model) has one take a model's: so
    /// that what names it reads, though its instances spawn nothing. It never gives way to its
    /// file, since a run that follows another takes no entities the prefab's file would give later.
    fn stand_in_prefab(&mut self, name: &str, path: Option<&str>, why: &Error) {
        let (path, named): (Arc<str>, Arc<str>) = match path {
            Some(path) => {
                let root = self.root.as_deref();
                let file = root.map_or(PathBuf::from(path), |root| root.join(path));
                (path.into(), self.texture_name(path, &file, None, None))
            }
            None => (name.into(), name.into()),
        };
        self.add_assets(|assets| {
            let prefab = assets.add_prefab(path.clone(), named, None);
            assets.alias_prefab(prefab, name.into());
        });
        self.added("prefab", &path);
        warn!(
            code = "asset.stand_in",
            name,
            path = &*path,
            cause = why.code(),
            "Prefab `{name}` can't be loaded, so a stand-in whose instances are their roots alone \
             takes its place, as the run follows another, which names it: {why:#}"
        );
    }

    /// What names the texture or the model loaded from `file`, which a layer gave as `path`, whose
    /// path among assets is `named`, as [`Assets`] says, or with `image`, its image of that place
    /// in the model: in a development build, the name given its path, if the run is given its
    /// assets' names, as [`name_as`](Self::name_as) says; and else its ID, as [`id_of`] finds it,
    /// with an image's place after the model's; and otherwise its path, as a build without `dev`
    /// that reads no package, as it reads no ID file, names every one.
    ///
    /// [`id_of`]: Self::id_of
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
        if let Some(given) = &self.given {
            return given.get(&at).cloned().unwrap_or(at);
        }
        match (self.id_of(path, file), image) {
            (Some(id), Some(i)) => format!("{id:016x}#{i}").into(),
            (Some(id), None) => format!("{id:016x}").into(),
            (None, _) => at,
        }
    }

    /// The ID of the asset at `path`, whose file is `file`: in a run that reads its game's package,
    /// the package's; else, in a development build, the asset database's, as
    /// [`asset_id`](Self::asset_id) finds it; and else none, as a build without `dev` reads no ID
    /// file.
    fn id_of(&self, path: &Path, file: Option<&Path>) -> Option<u64> {
        if let Some(package) = self.files.packaged() {
            return package.id(&self.files.within(path)?);
        }
        #[cfg(feature = "dev")]
        return self.asset_id(path, file).map(AssetId::get);
        #[cfg(not(feature = "dev"))]
        {
            let _ = file;
            None
        }
    }

    /// The texture loaded from `file`, which a layer gave as `path`, whose path among assets is
    /// `named`, with its size, if one was and isn't removed: by the file, as [`Loaded`] keeps it;
    /// or else, in a development build, the one loaded from the asset's file before it was renamed
    /// or moved, which its ID names, kept for `file` too from then on, so that an asset loads once
    /// whatever its file's been called.
    fn loaded_texture(
        &mut self,
        named: &str,
        path: &Path,
        file: Option<&Path>,
    ) -> Option<(Texture, [u32; 2])> {
        if let Some(&found) = file.and_then(|file| self.loaded.textures.get(file)) {
            return Some(found);
        }
        let id = self.texture_name(named, path, file, None);
        let texture = self
            .assets
            .live_texture(&id)
            .filter(|_| assets::is_id(&id))?;
        let loaded = self.loaded.textures.values();
        let found = loaded.copied().find(|&(loaded, _)| loaded == texture)?;
        if let Some(file) = file {
            self.loaded.textures.insert(file.to_owned(), found);
        }
        Some(found)
    }

    /// The model loaded from `file`, which a layer gave as `path`, whose path among assets is
    /// `named`, if one was, as [`loaded_texture`](Self::loaded_texture) finds a texture.
    fn loaded_model(&mut self, named: &str, path: &Path, file: Option<&Path>) -> Option<Model> {
        if let Some(&found) = file.and_then(|file| self.loaded.models.get(file)) {
            return Some(found);
        }
        let id = self.texture_name(named, path, file, None);
        let model = self.assets.live_model(&id).filter(|_| assets::is_id(&id))?;
        // Only one loaded from a file, which a stand-in wasn't.
        if !self.loaded.models.values().any(|&loaded| loaded == model) {
            return None;
        }
        if let Some(file) = file {
            self.loaded.models.insert(file.to_owned(), model);
        }
        Some(model)
    }

    /// The font loaded from `file`, which a layer gave as `path`, whose path among assets is
    /// `named`, if one was, as [`loaded_texture`](Self::loaded_texture) finds a texture.
    fn loaded_font(&mut self, named: &str, path: &Path, file: Option<&Path>) -> Option<Font> {
        if let Some(&found) = file.and_then(|file| self.loaded.fonts.get(file)) {
            return Some(found);
        }
        let id = self.texture_name(named, path, file, None);
        let font = self.assets.font(&id).filter(|_| assets::is_id(&id))?;
        // Only one loaded from a file, which a stand-in wasn't until its file came.
        if !self.loaded.fonts.values().any(|&loaded| loaded == font) {
            return None;
        }
        if let Some(file) = file {
            self.loaded.fonts.insert(file.to_owned(), font);
        }
        Some(font)
    }

    /// The prefab loaded from `file`, which a layer gave as `path`, whose path among assets is
    /// `named`, if one was, as [`loaded_texture`](Self::loaded_texture) finds a texture.
    fn loaded_prefab(&mut self, named: &str, path: &Path, file: Option<&Path>) -> Option<Prefab> {
        if let Some(&found) = file.and_then(|file| self.loaded.prefabs.get(file)) {
            return Some(found);
        }
        let id = self.texture_name(named, path, file, None);
        let prefab = self.assets.prefab(&id).filter(|_| assets::is_id(&id))?;
        // Only one loaded from a file, which a stand-in wasn't.
        if !self.loaded.prefabs.values().any(|&loaded| loaded == prefab) {
            return None;
        }
        if let Some(file) = file {
            self.loaded.prefabs.insert(file.to_owned(), prefab);
        }
        Some(prefab)
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
    fn add_assets<R>(&mut self, add: impl FnOnce(&mut Assets) -> R) -> R {
        // The worlds let go of their shares for the change.
        self.world.share_assets(&Arc::default());
        #[cfg(feature = "dev")]
        if let Some(edited) = &mut self.edited {
            edited.world.share_assets(&Arc::default());
        }
        let added = add(Arc::make_mut(&mut self.assets));
        self.world.share_assets(&self.assets);
        #[cfg(feature = "dev")]
        if let Some(edited) = &mut self.edited {
            edited.world.share_assets(&self.assets);
        }
        added
    }

    /// Has the world share the assets the layers added again, if a layer put another world in its
    /// place, as it loaded, attached or ticked, as [`World::share_assets`] says.
    pub(crate) fn share_assets(&mut self) {
        self.world.share_assets(&self.assets);
    }

    /// Scans the assets under the app's root, as [`assets::scan`] says, writing the ID files
    /// missing if `writes`, and warning of each missing otherwise, and has the worlds share what it
    /// found: as a test's context is given its root, and as the tool API lists them in a run that
    /// no thread watches them for. A context without a root has none, nor one that reads a package,
    /// whose assets have the IDs the package gives them. A texture is named by the ID
    /// the database holds for it as it's added, so that a scan after names none afresh.
    ///
    /// Each scan walks the whole root and reads every ID file on the main thread, O(files), and
    /// clones the catalog, O(assets): about 70 ms for 1,000 assets on Windows, but a few
    /// milliseconds apiece for ID files just copied in, the first time they're read; and each ID
    /// file it writes is flushed to the disk, about 6 ms apiece.
    #[cfg(feature = "dev")]
    pub(crate) fn scan_assets(&mut self, writes: bool) {
        self.scan_with(&mut Prints::default(), writes);
    }

    /// Scans the assets under the app's root, as [`scan_assets`](Self::scan_assets) does, pairing
    /// renames by `prints`.
    #[cfg(feature = "dev")]
    fn scan_with(&mut self, prints: &mut Prints, writes: bool) {
        // A run from a package reads its files there alone, and its assets' IDs.
        let Some(root) = self.root.as_deref().filter(|_| !self.reads_package()) else {
            return;
        };
        let _zone = gg_core::zone!();
        if let Some(catalog) = assets::scan(self.assets.catalog(), root, prints, writes) {
            self.set_catalog(catalog);
        }
    }

    /// Has the worlds share `catalog`, the asset database as a scan left it, and the thread that
    /// reloads assets follow each texture, model, font and prefab whose file it has elsewhere
    /// since, as [`HotReload::follow`] says.
    #[cfg(feature = "dev")]
    fn set_catalog(&mut self, catalog: Catalog) {
        self.add_assets(|assets| assets.set_catalog(catalog));
        let catalog = self.assets.catalog();
        self.uis.follow(|asset| {
            let id = asset
                .parse::<AssetId>()
                .ok()
                .filter(|_| assets::is_id(asset))?;
            catalog.path(id).map(Arc::from)
        });
        if let (Some(hot_reload), Some(root)) = (&mut self.hot_reload, &self.root) {
            hot_reload.follow(self.assets.catalog(), root);
        }
    }

    /// As a run starts, before the layers load, if a tool API `serves` the run, starts the thread
    /// that reloads the layers' shaders, textures, models, fonts, prefabs, sounds and music as
    /// their files change, as `hot_reload` says, warning if it can't; then scans the assets under
    /// the app's root, as [`scan_assets`](Self::scan_assets) does, writing the ID files missing if
    /// `writes`, and moving those that renames made since the last run left behind beside their
    /// assets, by the prints its cache keeps, as [`assets::scanned`] says; then, if the API serves
    /// the run, starts the thread that keeps the database from then on, as `asset_watch` says,
    /// warning if it can't, so that the API's `assets` scans on the main thread instead. A context
    /// without a root has no assets to scan, nor does a run that reads its game's package, which
    /// reloads none.
    #[cfg(feature = "dev")]
    pub(crate) fn start_assets(&mut self, serves: bool, writes: bool) {
        if self.reads_package() {
            return;
        }
        if serves {
            match HotReload::start() {
                Ok(hot_reload) => self.hot_reload = Some(hot_reload),
                Err(error) => warn!(
                    code = error.code(),
                    "{error:#}; assets don't reload as their files change"
                ),
            }
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let cache = self.asset_cache.clone().filter(|_| writes);
        let mut prints = match &cache {
            Some(cache) => Prints::read(cache, &root),
            None => Prints::default(),
        };
        self.scan_with(&mut prints, writes);
        if !serves {
            return;
        }
        let catalog = self.assets.catalog().clone();
        match AssetWatch::start(root, catalog, prints, writes, cache) {
            Ok(watch) => self.asset_watch = Some(watch),
            Err(error) => warn!(
                code = error.code(),
                "{error:#}; the tool API's `assets` scans the asset root itself"
            ),
        }
    }

    /// Takes the asset database the thread that watches the asset root sent since, if it changed,
    /// and has the worlds share it: as a frame starts, before the tool API's requests are answered.
    #[cfg(feature = "dev")]
    pub(crate) fn take_assets(&mut self) {
        let catalog = self.asset_watch.as_mut().and_then(AssetWatch::take);
        if let Some(catalog) = catalog {
            self.set_catalog(catalog);
        }
    }

    /// Sends the thread that reloads assets the shaders the layers added since, and puts each
    /// shader, texture, model, font, sound and music it read again since in place of the one
    /// loaded, under the same handle, as [`reload_texture`](Self::reload_texture),
    /// [`reload_model`](Self::reload_model) and [`reload_font`](Self::reload_font) say, which the
    /// next frame handed off draws, a sound's next play plays, logging `Sound reloaded`, and
    /// music's play crossfades to, logging `Music reloaded`: as a frame starts, `recorded` saying
    /// whether a recording follows the run. Once it has sent the shaders, it allocates nothing
    /// unless something came back.
    #[cfg(feature = "dev")]
    pub(crate) fn take_reloaded(&mut self, recorded: bool) {
        let Some(hot_reload) = &mut self.hot_reload else {
            return;
        };
        hot_reload.watch_shaders(&self.shaders);
        while let Some(reloaded) = self.hot_reload.as_ref().and_then(HotReload::take) {
            match reloaded {
                Reloaded::Shader(compiled) => {
                    let source = compiled.source();
                    let (shader, path) = (source.name(), source.path().display());
                    info!(shader, path = %path, "Shader reloaded");
                    self.shaders.replace(compiled);
                }
                Reloaded::Texture {
                    texture,
                    path,
                    image,
                } => self.reload_texture(texture, &path, image),
                Reloaded::Model {
                    file,
                    path,
                    imported,
                } => self.reload_model(&file, &path, imported, recorded),
                Reloaded::Font { font, path, bake } => self.reload_font(font, &path, *bake),
                Reloaded::Prefab { prefab, path, text } => {
                    self.reload_prefab(prefab, &path, text, recorded);
                }
                Reloaded::Ui { ui, path, document } => self.reload_ui(ui, &path, document),
                Reloaded::Sound { sound, path, data } => {
                    let named = assets::name_of(self.root.as_deref(), &path);
                    let frames = data.num_frames();
                    self.audio.replace(sound, *data);
                    info!(sound = &*named, frames, "Sound reloaded");
                }
                Reloaded::Music {
                    music,
                    path,
                    encoded,
                } => {
                    let named = assets::name_of(self.root.as_deref(), &path);
                    let frames = encoded.frames;
                    self.audio.replace_music(music, encoded);
                    info!(music = &*named, frames, "Music reloaded");
                }
            }
        }
    }

    /// Puts what `font` baked again from `path`, `bake`, in place of what it drew, its field taking
    /// the new one's texels at whatever size they are, logging `Font reloaded`, and has a later add
    /// of its file, by whatever path, give the font, as for a stand-in whose file came; but a
    /// stand-in keeps its place rather than take a file that came outside the asset root, as a
    /// texture's does, warning `asset.outside_root`. Text in it is laid out by what it baked from
    /// the next frame.
    #[cfg(feature = "dev")]
    fn reload_font(&mut self, font: Font, path: &Path, bake: font::Bake) {
        let named = assets::name_of(self.root.as_deref(), path);
        let was_loaded = self.loaded.fonts.values().any(|&loaded| loaded == font);
        if !was_loaded && let Err(error) = self.within_root(&named, path) {
            let consequence = "the stand-in keeps its place";
            warn!(
                code = error.code(),
                font = &*named,
                "{error:#}; {consequence}"
            );
            return;
        }
        let font::Bake {
            size,
            pixels,
            baked,
        } = bake;
        let field = self.fonts.field(font);
        let replaced = (self.textures).replace(&named, field, size, pixels, Texels::Data);
        if let Err(error) = replaced {
            let consequence = "the font keeps drawing as it did";
            warn!(
                code = error.code(),
                font = &*named,
                "{error:#}; {consequence}"
            );
            return;
        }
        self.fonts.set(font, baked);
        self.uis.font_changed(font);
        if let Some(file) = assets::file_of(path) {
            self.loaded.fonts.entry(file).or_insert(font);
        }
        info!(font = &*named, ?size, "Font reloaded");
    }

    /// Puts `image`, read again from `path`, in place of `texture`'s pixels, at whatever size it
    /// is, logging `Texture reloaded`, and has a later add of its file, by whatever path, give the
    /// texture, as for a stand-in whose file came. A sprite sheet's texture keeps drawing as it did
    /// rather than take an image of another size, whose cells would be elsewhere in it than its
    /// sprites have them, warning `asset.sheet_resized`; and a stand-in keeps its place rather
    /// than take a file that came outside the asset root, as through `..` or a link, as text's
    /// names are kept inside it, warning `asset.outside_root`. A texture removed since its file
    /// was read takes nothing. The image takes the place of any its loader was still decoding.
    #[cfg(feature = "dev")]
    fn reload_texture(&mut self, texture: Texture, path: &Path, image: Image) {
        if self.assets.removed(texture) {
            return;
        }
        let named = assets::name_of(self.root.as_deref(), path);
        let texels = image.texels(Texels::Color(Mipmaps::None));
        let Image { size, pixels, .. } = image;
        let loaded = self.loaded.textures.values();
        let was = loaded.copied().find(|&(loaded, _)| loaded == texture);
        // Only a stand-in was loaded from no file.
        if was.is_none()
            && let Err(error) = self.within_root(&named, path)
        {
            let consequence = "the stand-in keeps its place";
            warn!(
                code = error.code(),
                texture = &*named,
                "{error:#}; {consequence}"
            );
            return;
        }
        if let Some((_, was)) = was
            && was != size
            && self.loaded.sheets.contains(&texture)
        {
            warn!(
                code = "asset.sheet_resized",
                texture = &*named,
                "Texture `{named}` is {size:?} pixels across and down now, where it was {was:?}, \
                 but it's cut into a sprite sheet's cells, which a layer finds by its size as it \
                 adds it: it keeps drawing as it did until the run starts again"
            );
            return;
        }
        if let Err(error) = self.textures.replace(&named, texture, size, pixels, texels) {
            let consequence = "the texture keeps drawing as it did";
            warn!(
                code = error.code(),
                texture = &*named,
                "{error:#}; {consequence}"
            );
            return;
        }
        // In place of its pixels, if they're still decoding, which a replacement fills.
        self.loader.forget(texture);
        for (loaded, was) in self.loaded.textures.values_mut() {
            if *loaded == texture {
                *was = size;
            }
        }
        self.uis
            .texture_resized(texture, Vec2::new(size[0] as f32, size[1] as f32));
        if let Some(file) = assets::file_of(path) {
            self.loaded.textures.entry(file).or_insert((texture, size));
        }
        info!(texture = &*named, ?size, "Texture reloaded");
    }

    /// Puts what the model loaded from `file` draws, `imported`, read again from `path`, in place
    /// of what it drew, as [`model::reload`] says, logging `Model reloaded`: all of it, its roots'
    /// nodes fitted to its nodes as [`World::fit_nodes`] says, in the world and the world as edited,
    /// unless the run is `recorded`, or in a lockstep session or follows another, whose worlds
    /// change only as the run they go with changed them; there it takes its meshes and images
    /// alone, warning `asset.model_changed` of what changed that stays as it was until the run
    /// starts again, its nodes and materials, or of other numbers of primitives or images, which
    /// it doesn't take.
    #[cfg(feature = "dev")]
    fn reload_model(&mut self, file: &Path, path: &Path, imported: gltf::Imported, recorded: bool) {
        let Some(&model) = self.loaded.models.get(file) else {
            return;
        };
        let named = assets::name_of(self.root.as_deref(), path);
        let whole = !recorded && self.player.is_none() && !self.follows;
        let was = self.assets.nodes(model).expect("a model loaded").clone();
        let made = &mut self.loaded.made[model.index()];
        let stores = model::Stores {
            meshes: &mut self.meshes,
            shaders: &mut self.shaders,
            textures: &mut self.textures,
            materials: &mut self.materials,
        };
        let replaced = imported.images.len().min(made.images.len());
        let reloaded = model::reload(&named, imported, made, &was, whole, stores);
        let (nodes, kept, added) = match reloaded {
            Ok(reloaded) => reloaded,
            Err(error) => {
                warn!(
                    code = error.code(),
                    model = &*named,
                    "{error:#}; the model keeps drawing as it did"
                );
                return;
            }
        };
        // In place of their pixels, if they're still decoding, which the images replaced fill.
        for &(image, _) in &self.loaded.made[model.index()].images[..replaced] {
            self.loader.forget(image);
        }
        self.add_assets(|assets| {
            let (name, at) = (assets.model_name(model), assets.model_path(model));
            let images: Vec<_> = (added.iter())
                .map(|&(i, image)| {
                    (
                        image,
                        format!("{at}#{i}").into(),
                        format!("{name}#{i}").into(),
                    )
                })
                .collect();
            for (image, path, name) in images {
                assets.add_texture(image, path, name);
            }
        });
        if !whole && !kept.is_empty() {
            warn!(
                code = "asset.model_changed",
                model = &*named,
                "Model `{named}` reloaded its meshes and images, but its {} changed too, which the \
                 entities spawned from it keep as they were until the run starts again, as the run \
                 is recorded or in a lockstep session",
                kept.join(" and ")
            );
            return;
        }
        if nodes != was {
            self.add_assets(|assets| assets.set_nodes(model, nodes));
            self.world.fit_model(model, &was);
            if let Some(edited) = &mut self.edited {
                edited.world.fit_model(model, &was);
            }
        }
        info!(model = &*named, "Model reloaded");
    }

    /// Puts `text`, prefab `prefab`'s file read again from `path`, in place of what it held, and
    /// has every instance of it, in the world and the world as edited, take what changed, as
    /// [`World::refit_prefab`] says, logging `Prefab reloaded`; but where the run is `recorded`, in
    /// a lockstep session or follows another, whose worlds change only as the run they go with
    /// changed them, it keeps what it held, warning `asset.prefab_changed`. What the text names
    /// that isn't loaded is loaded first; text that doesn't load into the world, as one naming a
    /// type the world hasn't, or a prefab that links it, warns with its error's code, and the
    /// prefab and its instances stay as they were.
    #[cfg(feature = "dev")]
    fn reload_prefab(&mut self, prefab: Prefab, path: &Path, text: String, recorded: bool) {
        let named = assets::name_of(self.root.as_deref(), path);
        if recorded || self.player.is_some() || self.follows {
            warn!(
                code = "asset.prefab_changed",
                prefab = &*named,
                "Prefab `{named}` changed, which its instances keep as they were until the run \
                 starts again, as the run is recorded or in a lockstep session"
            );
            return;
        }
        if self
            .assets
            .prefab_text(prefab)
            .is_some_and(|was| **was == *text)
        {
            return;
        }
        let _ = self.load_scene_assets(&text);
        let text: Arc<str> = text.into();
        // Each world's found before either takes them, so that both take the text or neither does.
        let refits = self.world.refits(prefab, &text).and_then(|refits| {
            let edited = self.edited.as_ref();
            let edited = edited.map(|edited| edited.world.refits(prefab, &text));
            Ok((refits, edited.transpose()?))
        });
        let (refits, edited_refits) = match refits {
            Ok(refits) => refits,
            Err(error) => {
                warn!(
                    code = error.code(),
                    prefab = &*named,
                    "{error:#}; the prefab and its instances stay as they were"
                );
                return;
            }
        };
        self.world.refit_prefab(refits);
        if let (Some(edited), Some(refits)) = (&mut self.edited, edited_refits) {
            edited.world.refit_prefab(refits);
        }
        self.add_assets(|assets| assets.set_prefab_text(prefab, text));
        info!(prefab = &*named, "Prefab reloaded");
    }

    /// Puts `document`, UI file `file` read again from `path`, in place of what it held, every UI
    /// of it taking its nodes, as [`add_ui`](Self::add_ui) says, logging `UI reloaded`, unless it
    /// holds the same; what it names that isn't loaded is loaded first, and a font or a texture
    /// that doesn't load warns with its code, the UI staying as it was. A UI's state is the run's
    /// alone, so a recorded run or a lockstep peer takes it too.
    #[cfg(feature = "dev")]
    fn reload_ui(&mut self, file: usize, path: &Path, mut document: game_ui::Document) {
        let named = assets::name_of(self.root.as_deref(), path);
        if let Err(error) = self.load_ui_assets(&mut document) {
            warn!(
                code = error.code(),
                ui = &*named,
                "{error:#}; the UI stays as it was"
            );
            return;
        }
        if self.uis.holds(file, &document) {
            return;
        }
        self.uis.reload(file, document);
        info!(ui = self.uis.path(file), "UI reloaded");
    }

    /// Has the thread that reloads assets, if one does, watch UI file `file`, loaded as `name` from
    /// the file at `path` by a read begun at `read_at`, as [`watch_texture`](Self::watch_texture)
    /// has it watch a texture.
    #[cfg(feature = "dev")]
    fn watch_ui(&mut self, file: usize, name: &str, path: &Path, read_at: SystemTime, named: &str) {
        let id = named
            .parse::<AssetId>()
            .ok()
            .filter(|_| assets::is_id(named));
        let (id, at) = self.followed(id, path);
        if let Some(hot_reload) = &mut self.hot_reload {
            let source = Source::Ui {
                ui: file,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `prefab`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, as [`watch_texture`](Self::watch_texture) has
    /// it watch a texture.
    #[cfg(feature = "dev")]
    fn watch_prefab(
        &mut self,
        prefab: Prefab,
        name: &str,
        path: &Path,
        read_at: SystemTime,
        named: &str,
    ) {
        let id = named
            .parse::<AssetId>()
            .ok()
            .filter(|_| assets::is_id(named));
        let (id, at) = self.followed(id, path);
        if let Some(hot_reload) = &mut self.hot_reload {
            let source = Source::Prefab {
                prefab,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `texture`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, following its file as it's renamed or moved,
    /// as [`followed`](Self::followed) says, by its asset ID if it's named by one, as `named` is.
    #[cfg(feature = "dev")]
    fn watch_texture(
        &mut self,
        texture: Texture,
        name: &str,
        path: &Path,
        read_at: SystemTime,
        named: &str,
    ) {
        let id = named
            .parse::<AssetId>()
            .ok()
            .filter(|_| assets::is_id(named));
        let (id, at) = self.followed(id, path);
        if let Some(hot_reload) = &mut self.hot_reload {
            let source = Source::Texture {
                texture,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, put the file of the asset whose ID `name`
    /// is, if it's one, in place of `texture`, which stands in for it, once one has it.
    #[cfg(feature = "dev")]
    fn watch_for(&mut self, texture: Texture, name: &str) {
        let id = name.parse::<AssetId>().ok().filter(|_| assets::is_id(name));
        if let (Some(hot_reload), Some(id)) = (&mut self.hot_reload, id) {
            hot_reload.watch_for(HotAsset::Texture(texture), name, id);
        }
    }

    /// Has the thread that reloads assets, if one does, watch the model loaded as `name` from the
    /// file at `path`, which resolved to `file`, by a read begun at `read_at` that read `files`,
    /// following its file as it's renamed or moved, as [`followed`](Self::followed) says.
    #[cfg(feature = "dev")]
    fn watch_model(
        &mut self,
        file: &Path,
        name: &str,
        path: &Path,
        files: Vec<PathBuf>,
        read_at: SystemTime,
    ) {
        let (id, at) = self.followed(self.asset_id(path, Some(file)), path);
        if let Some(hot_reload) = &mut self.hot_reload {
            let source = Source::Model {
                file: file.to_owned(),
                name: name.to_owned(),
                path: path.to_owned(),
                files,
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `font`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, as [`watch_texture`](Self::watch_texture) has
    /// it watch a texture.
    #[cfg(feature = "dev")]
    fn watch_font(
        &mut self,
        font: Font,
        name: &str,
        path: &Path,
        read_at: SystemTime,
        named: &str,
    ) {
        let id = named
            .parse::<AssetId>()
            .ok()
            .filter(|_| assets::is_id(named));
        let (id, at) = self.followed(id, path);
        if let Some(hot_reload) = &mut self.hot_reload {
            let source = Source::Font {
                font,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `sound`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, at that path, as no asset ID names a sound.
    #[cfg(feature = "dev")]
    fn watch_sound(&mut self, sound: Sound, name: &str, path: &Path, read_at: SystemTime) {
        if let Some(hot_reload) = &mut self.hot_reload {
            let source = Source::Sound {
                sound,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, None, None);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `music`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, at that path, as no asset ID names music.
    #[cfg(feature = "dev")]
    fn watch_music(&mut self, music: Music, name: &str, path: &Path, read_at: SystemTime) {
        if let Some(hot_reload) = &mut self.hot_reload {
            let source = Source::Music {
                music,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, None, None);
        }
    }

    /// Has the thread that reloads assets, if one does, bake the file of the asset whose ID `name`
    /// is, if it's one, in place of `font`, which stands in for it, once one has it.
    #[cfg(feature = "dev")]
    fn watch_font_for(&mut self, font: Font, name: &str) {
        let id = name.parse::<AssetId>().ok().filter(|_| assets::is_id(name));
        if let (Some(hot_reload), Some(id)) = (&mut self.hot_reload, id) {
            hot_reload.watch_for(HotAsset::Font(font), name, id);
        }
    }

    /// What the thread that reloads assets follows the file at `path` by as it's renamed or moved,
    /// as [`HotReload::watch`] takes it: its asset ID, `id`, if it has one, and its path from the
    /// asset root, as the asset database has it by that ID, or else as `path` gives it, or as the
    /// OS resolves it, if it's under the root, by which the database gives it an ID once it has
    /// one, as for a file new to the run; none outside the root.
    #[cfg(feature = "dev")]
    fn followed(&self, id: Option<AssetId>, path: &Path) -> (Option<AssetId>, Option<Arc<str>>) {
        let by_id = id
            .and_then(|id| self.assets.catalog().path(id))
            .map(Arc::from);
        let at = by_id.or_else(|| {
            let root = self.root.as_deref()?;
            let spelled = path
                .strip_prefix(root)
                .ok()
                .map(|within| assets::name_of(None, within));
            spelled.or_else(|| {
                let within = assets::file_of(path)?;
                Some(assets::name_of(
                    None,
                    within.strip_prefix(assets::file_of(root)?).ok()?,
                ))
            })
        });
        (id, at)
    }

    /// Whether a thread watches the asset root, keeping the database.
    #[cfg(feature = "dev")]
    pub(crate) fn watches_assets(&self) -> bool {
        self.asset_watch.is_some()
    }

    /// Asks the thread that watches the asset root to look now, as the tool API's `assets` does;
    /// returns the look's number, which [`assets_answered`](Self::assets_answered) reaches once
    /// it's taken; `None` in a run that no thread watches the root for.
    #[cfg(feature = "dev")]
    pub(crate) fn ask_assets(&self) -> Option<u64> {
        self.asset_watch.as_ref().map(AssetWatch::ask)
    }

    /// The looks [`ask_assets`](Self::ask_assets) asked for that have been taken: those up to it.
    #[cfg(feature = "dev")]
    pub(crate) fn assets_answered(&self) -> u64 {
        self.asset_watch
            .as_ref()
            .map_or(u64::MAX, AssetWatch::answered)
    }

    /// The asset database, as the last scan left it.
    #[cfg(feature = "dev")]
    pub(crate) fn catalog(&self) -> &Catalog {
        self.assets.catalog()
    }

    /// Takes the asset of `kind` the run has just written at `path`, with its ID file holding `id`,
    /// into the asset database, as [`Catalog::bind`] does, and asks the thread that watches the
    /// root to look, so that its database takes it too.
    #[cfg(feature = "dev")]
    pub(crate) fn bind_asset(&mut self, path: Arc<str>, kind: AssetKind, id: AssetId) {
        self.add_assets(|assets| assets.bind(path, kind, id));
        self.ask_assets();
    }

    /// Adds the prefab of `text`, which the run has just written in a new file at `path` from the
    /// asset root, with `/` between its parts, named `name`, its asset ID or else its path: as
    /// [`add_prefab`](Self::add_prefab) adds one it reads, watched as its file changes, but a new
    /// prefab whatever was loaded from a file there before, as one deleted since, which neither
    /// `path` nor the file names from now on.
    #[cfg(feature = "dev")]
    pub(crate) fn add_saved_prefab(&mut self, path: &str, name: &str, text: String) -> Prefab {
        // The file holds the text from now on, as a read begun now would give it.
        let read_at = SystemTime::now();
        let file = match &self.root {
            Some(root) => root.join(path),
            None => PathBuf::from(path),
        };
        let text: Arc<str> = text.into();
        let (path, name): (Arc<str>, Arc<str>) = (path.into(), name.into());
        let (held, named) = (Arc::clone(&text), name.clone());
        let prefab = self.add_assets(|assets| assets.add_saved_prefab(path.clone(), named, held));
        self.watch_prefab(prefab, &path, &file, read_at, &name);
        if let Some(resolved) = self.files.file_of(&file) {
            self.loaded.prefabs.insert(resolved, prefab);
        }
        self.added("prefab", &path);
        // What its text names, loaded already, as the entities it was saved from have it.
        let _ = self.load_scene_assets(&text);
        prefab
    }

    /// Whether a layer has added the file at `path` from the app's root, by whatever form of its
    /// path, as a texture, a sprite sheet or a model, or a texture its asset's ID names is loaded,
    /// as one loaded from it before it was renamed is, and not removed since: resolving the file,
    /// some 0.1 ms on Windows.
    #[cfg(feature = "dev")]
    pub(crate) fn has_added(&self, path: &str) -> bool {
        let file = (self.root.as_deref()).and_then(|root| assets::file_of(&root.join(path)));
        let by_file = file.is_some_and(|file| {
            self.loaded.textures.contains_key(&file) || self.loaded.models.contains_key(&file)
        });
        let id = self.assets.catalog().id(path);
        by_file || id.is_some_and(|id| self.assets.live_texture(&id.to_string()).is_some())
    }

    /// Draws `mesh` with `shader` this frame, where `transform` places it in the world, in the
    /// game's view as the camera sees it: with what's opaque, first, hiding what's behind it, if
    /// `shader` is opaque, or else over what's drawn before it and what's opaque, but where what's
    /// opaque is nearer; the shader gets `params`. The view is the main window's central area, which the
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
        let transform = relative(transform, self.origin);
        self.draws.push(mesh, shader, transform, params);
    }

    /// Draws `sprite` this frame, where `transform` places it in the world, over what's drawn
    /// before it and what's opaque, but where what's opaque is nearer, in the game's view as the
    /// camera sees it.
    /// Unscaled, a sprite is a unit square around the transform's translation, as [`Sprite`] says.
    /// It's of no entity, so a [pick](Self::pick) where it's on top finds none.
    pub fn draw_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        (self.draws).push_sprite(relative(transform, self.origin), sprite);
    }

    /// Draws `circle` this frame, where `transform` places it in the world, over what's drawn
    /// before it and what's opaque, but where what's opaque is nearer, in the game's view as the
    /// camera sees it.
    /// Unscaled, a circle is a unit across, around the transform's translation, as [`Circle`]
    /// says. It's of no entity, so a [pick](Self::pick) where it's on top finds none.
    pub fn draw_circle(&mut self, transform: Mat4, circle: Circle) {
        let transform = relative(transform, self.origin);
        self.draws.push_circle(&mut self.shaders, transform, circle);
    }

    /// Draws `text` this frame in `style`, where `transform` places its origin in the world, over
    /// what's drawn before it and what's opaque, but where what's opaque is nearer, in the game's
    /// view as the camera sees it: a line for each of its own, which `\n` ends, or `\r\n`, laid out left to
    /// right, each of its characters a glyph of the style's font as its kerning places them, shaped
    /// without the features that put glyphs in place of others, as ligatures, its em `style.size`
    /// of the transform's units, and its origin the point of its box `style.align` says, in the
    /// transform's XY plane, +Y up, facing +Z, as a sprite is. Each glyph is drawn crisp at any
    /// size from the font's field, as [`Glyph`](gg_renderer::Glyph) says, by straight alpha, and
    /// it's of no entity, so a [pick](Self::pick) where it's on top finds none. A style that names
    /// no font draws in the engine's own, Inter, as the UI does, which bakes the first time text is
    /// drawn in it, about 50 ms in release and 140 ms in a debug build. Once a text as long has
    /// been drawn, drawing text allocates nothing.
    ///
    /// # Panics
    ///
    /// If `style.font` is another context's.
    pub fn draw_text(&mut self, transform: Mat4, text: &str, style: &TextStyle) {
        let transform = relative(transform, self.origin);
        let glyphs = self.fonts.lay_out(transform, text, style, 0);
        self.draws.extend_glyphs(&mut self.shaders, glyphs);
    }

    /// Draws a line from `from` to `to` in the world this frame, `width` pixels of the view wide
    /// however near or far it is, or, thinner than a pixel, a pixel wide and as much fainter as
    /// it's thinner, its ends squared off half its width past its points, in `color`, linear RGBA
    /// with alpha not multiplied in: over everything else the view draws, the world's entities as
    /// well as what layers draw, but where an opaque mesh is nearer, so that it shows as debugging
    /// wants. It's of no entity, so a [pick](Self::pick) where it's on top finds none.
    pub fn draw_line(&mut self, from: Vec3, to: Vec3, color: Vec4, width: f32) {
        let origin = self.origin.as_vec3();
        let line = Line {
            from: from - origin,
            to: to - origin,
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
        let transform = relative(transform, self.origin);
        self.over
            .push_rect(&mut self.shaders, transform, color, width, 0);
    }

    /// Draws on the game's view's canvas this frame, from `anchor`: a game's own UI, its HUDs and
    /// menus, apart from the development UI, which a layer lays out as it updates, each frame, in
    /// the view's own space. `anchor` is one of the view's edges, its corners or its middle, across
    /// then down, as text's alignment says, such as `[Align::End, Align::Start]` for its top right,
    /// which the canvas's origin is at, +Y up; and the canvas scales with the view,
    /// [`CANVAS_HEIGHT`](crate::CANVAS_HEIGHT) units to its height, as the frame handed off finds
    /// it, whatever its size in pixels. What's drawn on it draws over everything else in the view,
    /// the world's nearest entities and lines too, in the order it's drawn, so that captures and
    /// the editor's view show it, but under the development UI. Its state, such as which button has
    /// the focus, is the layer's, so that each lockstep peer has its own, and a choice that changes
    /// the game queues a command, as input does.
    pub fn canvas(&mut self, anchor: [Align; 2]) -> Canvas<'_> {
        let (view, cursor) = (self.view, self.input.cursor());
        let shaders = &mut self.shaders;
        Canvas::new(
            &mut self.canvas,
            &mut self.fonts,
            shaders,
            anchor,
            view,
            cursor,
        )
    }

    /// Whether the cursor is over what a game UI draws that a pointer works on, in any UI that
    /// takes input and drew in the last frame: a button, a widget or a list, within what clips it,
    /// or anywhere while a drop-down's options are open, as [`GameUi::pointer_over`] says of one.
    /// A game that reads the mouse itself, to aim, pick or drag, leaves it alone while it's over.
    pub fn pointer_over_ui(&mut self) -> bool {
        let (view, cursor) = (self.view, self.input.cursor());
        self.uis.pointer_over(&mut self.fonts, view, cursor)
    }

    /// What the UIs the layers added lay out, draw and take input with, lent apart: the UIs, the
    /// fonts their texts are measured and drawn in, the canvas and the shaders it draws with,
    /// where the view was as the last frame handed it off, and the input as the layers are sent
    /// it.
    pub(crate) fn ui_parts(&mut self) -> game_ui::Parts<'_> {
        game_ui::Parts {
            uis: &mut self.uis,
            fonts: &mut self.fonts,
            drawn: &mut self.canvas,
            shaders: &mut self.shaders,
            view: self.view,
            input: &self.input,
        }
    }

    /// Each UI the layers added, laid out for the view as the last frame handed it off, as the
    /// tool API's `ui` lists them.
    #[cfg(feature = "dev")]
    pub(crate) fn ui_list(&mut self) -> gg_api::UiList {
        self.uis.listed(&mut self.fonts, self.view)
    }

    /// Ends the run before its next frame, as closing its window does when no layer handles that:
    /// a game's own Quit, such as its menu's.
    pub fn exit(&mut self) {
        self.exiting = true;
    }

    /// Whether a layer has asked the run to end, as [`exit`](Self::exit) and [`fail`](Self::fail)
    /// say.
    pub fn exiting(&self) -> bool {
        self.exiting
    }

    /// Ends the run before its next frame, as [`exit`](Self::exit) does, but with `error`, which it
    /// logs as an error, by its code, at once: for what a layer can't go on without, such as a
    /// script or a scene that doesn't load, in place of a panic. The layers still detach, and the
    /// run logs `engine.perf` as it ends, but not `Exited cleanly`, and its status is 1. A replay,
    /// which goes as its recording does, logs the error too, its status saying whether its hashes
    /// matched.
    pub fn fail(&mut self, error: Error) {
        error!(code = error.code(), "{error:#}");
        self.exiting = true;
        self.failed = true;
    }

    /// Draws a grid this frame as `viewpoint` sees it, fitted to the game's view as the last frame
    /// handed it off, which [`view`](Self::view) gives: lines a power of ten apart on the world's
    /// axis plane `plane`, or on the one an orthographic viewpoint along an axis faces, as
    /// [`grid::shown`] says, and the plane's two axes in their colors, [`grid::RED`],
    /// [`grid::GREEN`] and [`grid::BLUE`], as the editor's view shows them. The lines show once
    /// they're 8 pixels apart, are crisp, a pixel each, through an orthographic viewpoint that's
    /// [`grid::square`], and fade toward grazing angles. It draws by straight alpha over what's
    /// drawn before it and what's opaque, but where what's opaque is nearer, and is of no entity, so
    /// drawn before the world, which the engine draws after the layers' draws, it leaves picks to
    /// what's on top.
    /// `viewpoint` is the one the frame sees from, as [`set_viewpoint`](Self::set_viewpoint) sets
    /// it. Nothing draws while the view has no area, as in a dockspace's first frame, or while the
    /// plane is too near edge-on to show.
    pub fn draw_grid(&mut self, viewpoint: &Viewpoint, plane: Plane) {
        let Some(view) = self.view else {
            return;
        };
        let shown = grid::shown(viewpoint, plane);
        if let Some((transform, grid)) = grid::quad(viewpoint, view, shown, self.origin) {
            (self.draws).push_grid(&mut self.meshes, &mut self.shaders, transform, grid);
        }
    }

    /// Outlines what the game's view shows of entity `id` and of every entity under it this frame,
    /// as the editor outlines what's picked, a model's nodes or a prefab's entities among them:
    /// round whatever shape the pixels make together where their draws are on top, their
    /// colliders' outlines among them, even across something in front of them, but not round
    /// what's hidden; over what the view shows and under the UI, in `color`, linear RGBA with
    /// alpha not multiplied in: a solid band `width` pixels of the view wide, at most 8, its outer
    /// edge smoothed, then a glow at up to half its alpha that fades over as far again. An entity
    /// that's gone outlines nothing. One entity and its subtree a frame: a later call replaces an
    /// earlier one's.
    pub fn outline(&mut self, id: EntityId, color: Vec4, width: f32) {
        self.outline_root.clear();
        self.outline_root.push(id);
        self.outline = Some((color, width));
    }

    /// What a layer has [outlined](Self::outline) this frame, if anything: the entity, the color
    /// and the width.
    pub fn outlined(&self) -> Option<(EntityId, Vec4, f32)> {
        let (color, width) = self.outline?;
        Some((*self.outline_root.first()?, color, width))
    }

    /// Whether the world's colliders' outlines draw, as
    /// [`set_colliders_shown`](Self::set_colliders_shown) says: not at first.
    pub fn colliders_shown(&self) -> bool {
        self.colliders
    }

    /// Has the world's colliders' outlines draw each frame from then on, or not: each
    /// [`BoxCollider2D`], [`CircleCollider2D`], [`BoxCollider3D`], [`SphereCollider3D`] and
    /// [`CapsuleCollider3D`] whose entity has a [`Transform`], as physics shapes it, 2 points wide
    /// in light green, where its entity draws, over everything else the view draws but a nearer
    /// opaque mesh, as lines are; with its entity's ID, so that a [pick](Self::pick) of an outline
    /// finds its entity. A collider's outline draws whether physics moves its entity or not, as it
    /// doesn't without a [`RigidBody2D`] or a [`RigidBody3D`], or under a parent.
    pub fn set_colliders_shown(&mut self, shown: bool) {
        self.colliders = shown;
    }

    /// Whether the world's navmesh and its agents' paths draw, as
    /// [`set_navmesh_shown`](Self::set_navmesh_shown) says: not at first.
    pub fn navmesh_shown(&self) -> bool {
        self.navmesh
    }

    /// Has the world's navmesh and its agents' paths draw each frame from then on, or not, as the
    /// last [`step_navigation`](crate::step_navigation) left them: each of the navmesh's polygons
    /// outlined in sky blue along its cells' floors, with its [`NavMeshVolume`]'s entity's ID, and
    /// each [`NavAgent`]'s path in amber from where its entity draws through the corners it has
    /// left, its destination marked, with the agent's ID, so that a [pick](Self::pick) of a line
    /// finds its entity; 2 points wide, a little above the floors, over everything else the view
    /// draws but a nearer opaque mesh, as lines are.
    pub fn set_navmesh_shown(&mut self, shown: bool) {
        self.navmesh = shown;
    }

    /// The world: its entities and their components, as the frame's ticks left it. After the
    /// layers' updates each frame, the engine draws those with a [`Transform`] and a [`Sprite`], a
    /// [`Circle`], a [`Text`](crate::Text) or a [`MeshDraw`], after what the layers drew
    /// themselves: the meshes whose shaders are opaque first, whatever their z's, by shader, then
    /// mesh, then the rest back to front by their z's in the world, at one z sprites, then circles,
    /// then text, under meshes, each in the order of their IDs, children where their parents carry
    /// them; each on its way from where it was as the latest tick started to where the tick left
    /// it, so up to a tick behind the world here.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The meshes the layers added, which the tool API reads their extents from.
    #[cfg(feature = "dev")]
    pub(crate) fn meshes(&self) -> &Meshes {
        &self.meshes
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
    /// each texture, model, font and prefab it names, by its ID or its path from the asset root,
    /// that no layer has added, as [`add_texture`](Self::add_texture),
    /// [`add_model`](Self::add_model), [`add_font`](Self::add_font) and
    /// [`add_prefab`](Self::add_prefab) load a file, a model's image with its model. A development
    /// build watches the file
    /// from then on: saved while the game runs, it applies to the world as the operations that take
    /// the world's entities from what the file held to what it holds, between ticks, which keeps
    /// what the file doesn't change, as where the game has moved an entity; an edit whose author is
    /// the file's path, as the app's root names it, such as `scenes/level.ron`, which the tool
    /// API's `undo` takes back. A run restored from a snapshot watches the files its run had
    /// loaded. The project's start scene, as [`load_start_scene`](Self::load_start_scene) loads it,
    /// is the open scene too. It logs `scene.loaded`, with how many roots it made and how long it
    /// took, the loads of what it names among it.
    ///
    /// Fails as `read_scene` does, and as `load_scene` does, the path beginning its message, as for
    /// a texture, a model, a font or a prefab that can't be loaded, `asset.no_texture`,
    /// `asset.no_model`, `asset.no_font` or `asset.no_prefab`, with why: an image its model hasn't,
    /// or one named by an absolute path or one leading outside the asset root, by an ID no asset
    /// has, or by a path to a file that doesn't load as `add_texture`, `add_model` or `add_font`
    /// loads it. In a replay or a lockstep joiner, which goes as another run went, such an asset
    /// stands in, warning `asset.stand_in`.
    ///
    /// # Panics
    ///
    /// Outside the layers' attach and detach, as [`world_mut`](Self::world_mut) does.
    pub fn load_scene_file(&mut self, path: impl AsRef<Path>) -> Result<Vec<Entity>> {
        let started = Instant::now();
        let path = path.as_ref();
        let text = crate::ecs::read_scene_from(path, &self.files)?;
        let mut loaded = self.world_mut().load_scene(&text);
        // Read again once what it names that isn't loaded is, which is seldom, so that a scene that
        // names what's loaded is read once.
        if loaded.as_ref().is_err_and(|error| {
            matches!(
                error.code(),
                "asset.no_texture" | "asset.no_model" | "asset.no_font" | "asset.no_prefab"
            )
        }) {
            loaded =
                (self.load_scene_assets(&text)).and_then(|_| self.world_mut().load_scene(&text));
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
        let file = SceneFile::new(path.to_owned(), author, text);
        // For the scene changes to come, which may name it.
        self.scenes.cache(file.clone());
        self.scenes.loaded(file);
        info!(
            code = "scene.loaded",
            path = %path.display(),
            roots = roots.len(),
            ms = perf::millis(started.elapsed()),
            "Loaded the scene file"
        );
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

    /// Drops the scene a layer asked for outside a tick, through [`World::change_scene`], as in its
    /// `update`, `event` or `attach`, warning `scene.not_in_tick`: as each tick starts, before the
    /// layers' ticks, so that only what a tick asks for goes in, as a recording and the other
    /// lockstep peer have it.
    pub(crate) fn drop_untimely_scene(&mut self) {
        if let Some(scene) = self.world.take_asked_scene() {
            warn!(
                code = "scene.not_in_tick",
                scene = &*scene,
                "A layer asked for the scene {scene} outside its tick, where the world doesn't \
                 change, so it's dropped: a layer asks in its tick, as for a command it queued"
            );
        }
    }

    /// Puts the scene the world's last tick asked for, through [`World::change_scene`], in place
    /// of the world's entities, as [`change_scene`](Self::change_scene) does, if it asked for one:
    /// as each layer's tick returns, before the next layer's, wherever a tick runs, again in a
    /// rollback or a SyncTest too. Logs, at the debug level, `scene.entered`, with how many roots
    /// the scene made and how long it took, the loads of what it names among it; or the error,
    /// the world staying as it was, which the world notes, for
    /// [`confirm_scene`](Self::confirm_scene) to log once the tick is final, as a tick that may run
    /// again, a client's or a rollback peer's on the other's inputs predicted, would log it again,
    /// and one that never does would log a refusal that didn't happen.
    pub(crate) fn change_asked_scene(&mut self) {
        let Some(scene) = self.world.take_asked_scene() else {
            return;
        };
        let started = Instant::now();
        match self.change_scene(&scene) {
            Ok(roots) => debug!(
                code = "scene.entered",
                scene = &*scene,
                roots,
                ms = perf::millis(started.elapsed()),
                "Put the scene's entities in place of the world's"
            ),
            Err(error) => {
                debug!(
                    refused = error.code(),
                    scene = &*scene,
                    "{error:#}; the world stays as it was"
                );
                self.world.refuse_scene(&scene, &error);
            }
        }
    }

    /// Puts the scene file `scene` names, its path from the asset root, `/` between its parts, in
    /// place of the world's entities, as [`World::change_scene`] says, and returns how many roots
    /// it made: its text as [`scene_text`](Self::scene_text) keeps it; each model and prefab it
    /// names that no layer has added loaded first, none standing in, and then each texture and
    /// font, a stand-in for one that can't be loaded, as a run that follows another stands in, so
    /// that whether the scene goes in depends on its text and the models and prefabs alone; in a
    /// server's session, server and clients alike, none, as a client reads its server's states
    /// only of the assets loaded.
    ///
    /// Fails, changing nothing, with `scene.bad_path` for a name that isn't a path inside the asset
    /// root, as one that's absolute, holds `.`, `..`, `\`, `:` or a control character, or is
    /// empty; as `scene_text` does; `asset.no_model` or `asset.no_prefab`, saying why, for a model
    /// or a prefab it names that doesn't load; `net.no_scene_change` in a server's session for a
    /// scene that names an asset the layers haven't loaded; and as [`World::load_scene`] does into
    /// the world without its entities, the name beginning the message.
    pub(crate) fn change_scene(&mut self, scene: &str) -> Result<usize> {
        let _zone = gg_core::zone!();
        if !package::inside(scene) || scene.chars().any(char::is_control) {
            let message = format!(
                "`{scene}` isn't a scene file's path from the asset root, `/` between its parts, \
                 none of them empty, `.` or `..`, nor holding `\\`, `:` or a control character, \
                 which a game names its scenes by on every OS and in its package alike"
            );
            return Err(Error::new("scene.bad_path", message));
        }
        let text = self.scene_text(scene)?;
        self.load_assets_of(scene, &text)?;
        let roots = self.world.change_entities(scene, &text).map_err(|error| {
            let message = format!("{scene}:{error}");
            error.with_message(message)
        })?;
        Ok(roots.len())
    }

    /// Loads what scene `text`, of the scene file `scene` names, names that no layer has added, as
    /// [`change_scene`](Self::change_scene) says, the world sharing what's loaded; in a server's
    /// session none, refusing a scene that names what no layer loaded.
    ///
    /// Fails as `change_scene` does for what the scene names, the name beginning the message.
    pub(crate) fn load_assets_of(&mut self, scene: &str, text: &str) -> Result<()> {
        // A world put in the world's place, as a SyncTest's copy, may share fewer assets than the
        // layers have added since it was taken, which the scene may name.
        self.share_assets();
        let named = |error: Error| {
            let message = format!("{scene}:{error}");
            error.with_message(message)
        };
        if !self.served {
            return self.load_change_assets(text).map_err(named);
        }
        let missing = self.world.scene_assets(text).map_err(named)?;
        if let Some(asset) = missing.first() {
            let message = format!(
                "{scene} names {}, which no layer has loaded, and a server's session loads none as \
                 it plays, so that its clients read its states: load what the game's scenes name as \
                 its layers load",
                asset.name()
            );
            return Err(Error::new("net.no_scene_change", message));
        }
        Ok(())
    }

    /// Makes the cells of a world whose [`Cells`](crate::Cells) are of folder `folder` that have no
    /// file, from `from` to `to` on each axis, as `text` gives each: a cell's scene text, as its
    /// file would hold it. So a world reaches as far as the game makes it, without a file for each
    /// cell; a cell's file stands for the game's making of it, as a development build's save writes
    /// one for a made cell that changed. A layer makes them in its
    /// [`load`](crate::Layer::load), which a restart and a replay run too; again, for the same
    /// folder, in place of what it made before.
    ///
    /// `text` runs on a thread that prepares cells, as a viewer comes near, and again whenever the
    /// cell comes in from it, so it reads nothing but its cell, and gives each cell the same text
    /// each time and on every platform, as the world's ticks must; and the IDs its entities have
    /// are theirs alone. A made cell comes in, and goes out keeping nothing as it came, as one of a
    /// file does, its text's digest noted in the world, but neither kept by the run nor written to
    /// a recording, which the world's hash checks in its place. The cells within each viewer's
    /// radius and a cell and a half are looked for, each tick, in a box of at most 32,768 cells
    /// round the viewer: so a world one cell deep reaches some 90 cells either way, and the 4,096
    /// nearest, a world's most at once, are always among them.
    pub fn make_cells(
        &mut self,
        folder: &str,
        from: Cell,
        to: Cell,
        text: impl Fn(Cell) -> String + Send + Sync + 'static,
    ) {
        let made = MadeCells {
            from,
            to,
            text: Arc::new(text),
        };
        let run = &mut self.cells.made;
        match run.iter_mut().find(|(was, _)| was == folder) {
            Some((_, was)) => *was = made,
            None => run.push((folder.to_owned(), made)),
        }
    }

    /// Takes the world's cells out and finds those coming in, as [`Cells`](crate::Cells),
    /// [`Viewer`](crate::Viewer) and [`Streamed`](crate::Streamed) say, as each tick ends, after
    /// its layers', wherever a tick runs, again in a rollback or a SyncTest too: from the world
    /// alone, as [`World::plan_cells`] finds it, so that every run of the tick does the same. Cells
    /// go out first, in order, then the roots left in cells out; and the cells coming in are noted
    /// in the world, which the next tick brings in as it starts, as
    /// [`enter_coming`](Self::enter_coming) says, so that a tick whose cells aren't ready can wait
    /// for them before it runs.
    ///
    /// Logs, at the debug level, `cells.left`, with the cell, how many roots went and how long it
    /// took. A cell whose roots can't be kept as text stays in, warning `cells.held`. A server's
    /// client takes none out: its cells come and go with its server's states. A world without
    /// `Cells` costs a query; one whose cells stay as they are, the plan, whose room [`CellRun`]
    /// keeps from tick to tick.
    pub(crate) fn stream_cells(&mut self) {
        if self.client {
            return;
        }
        let Some((id, cells, count)) = self.world.world_cells() else {
            return;
        };
        let tie = (count > 1).then_some((id, count));
        if tie.is_some() && tie != self.cells.tie {
            warn!(
                code = "cells.tie",
                seen = %id,
                count,
                "{count} entities have Cells: the world streams by the one of the lowest ID, {id}"
            );
        }
        self.cells.tie = tie;
        let size = cells.size;
        if !(size.is_finite() && size > 0.0) {
            if self.cells.first("cells.bad_size", id) {
                warn!(
                    code = "cells.bad_size",
                    entity = %id,
                    size,
                    "The Cells of entity {id} are {size} units on a side, which isn't finite and \
                     positive, so no cell comes in or goes out"
                );
            }
            return;
        }
        let run = &mut self.cells;
        let listed = run.listed(self.root.as_deref(), &self.files, &cells.folder);
        let made = run.made(&cells.folder).map(|made| (made.from, made.to));
        self.world
            .plan_cells(size, &listed, made, None, &mut run.plan);
        let plan = mem::take(&mut run.plan);
        // Those no viewer is near any more.
        (run.preparing.cells).retain(|cell, _| plan.nearby.binary_search(cell).is_ok());
        let size = f64::from(size);
        let unprepared = |prepared: &BTreeMap<Cell, Prep>, &(past, cell): &(f64, Cell)| {
            past <= size && !prepared.contains_key(&cell)
        };
        let prepared = &self.cells.preparing.cells;
        if plan.is_empty() && !plan.ahead.iter().any(|ahead| unprepared(prepared, ahead)) {
            self.cells.plan = plan;
            return;
        }
        let folder = cells.folder.clone();
        let of = roots_of;
        for &cell in &plan.leaving {
            let started = Instant::now();
            let roots = of(&plan.leavers, cell);
            match self.world.keep_cell(cell, &roots, true) {
                Ok(roots) => debug!(
                    code = "cells.left",
                    cell = %cell,
                    roots,
                    ms = perf::millis(started.elapsed()),
                    "A cell went out"
                ),
                Err(error) => self.held(cell, &error),
            }
        }
        let mut strays = plan.strays.iter().map(|&(cell, _)| cell).peekable();
        while let Some(cell) = strays.next() {
            while strays.next_if_eq(&cell).is_some() {}
            let roots = of(&plan.strays, cell);
            match self.world.keep_cell(cell, &roots, false) {
                // What was prepared stands for what it kept before.
                Ok(_) => drop(self.cells.preparing.cells.remove(&cell)),
                Err(error) => self.held(cell, &error),
            }
        }
        self.world.note_coming(&plan.entering);
        self.world.forget_refused(&plan.forgotten);
        if plan.full && self.cells.first("cells.full", &folder) {
            let most = cells::MOST_CELLS;
            warn!(
                code = "cells.full",
                folder = &*folder,
                most,
                "More cells are near the viewers than the {most} a world holds at once: the \
                 nearest come in"
            );
        }
        let prepared = &self.cells.preparing.cells;
        let prepare = plan
            .ahead
            .iter()
            .filter(|ahead| unprepared(prepared, ahead));
        let prepare: Vec<Cell> = prepare.map(|&(_, cell)| cell).collect();
        for cell in prepare {
            let source = self.cells.source(&folder, &listed, cell);
            self.start_preparing(&folder, cell, source);
        }
        self.cells.plan = plan;
    }

    /// The cells' step while the run is stopped, as a world is edited, each frame, since no tick
    /// runs: as [`stream_cells`](Self::stream_cells) does as a tick ends, the look's viewpoint a
    /// viewer too, its radius the largest of the world's viewers' and [`VIEWED_RADIUS`]; but the
    /// cells coming in come once all of them are prepared, as [`cells_ready`](Self::cells_ready)
    /// prepares them, no frame waiting; a cell that a streamed root is in comes in wherever it is,
    /// so that a root moved into a cell brings the cell's file in with it, rather than going out
    /// into what the cell keeps; and a cell no viewer is near goes out only as its file has it, its
    /// roots written as a world's save would write its file the same as the file as last applied,
    /// as [`World::cell_written`] says, keeping nothing, as [`World::forget_cell`] says, logging
    /// `cells.left` at the debug level. So what's edited stays in until it's saved, and nothing
    /// edited is kept as text, which a save would have to write. A server's client streams none.
    #[cfg(feature = "dev")]
    pub(crate) fn stream_stopped(&mut self) {
        if self.client {
            return;
        }
        let Some((_, cells, _)) = self.world.world_cells() else {
            return;
        };
        let size = cells.size;
        // The tick's step warns of it, as the copy plays.
        if !(size.is_finite() && size > 0.0) {
            return;
        }
        let folder = cells.folder.clone();
        // Every frame, as every tick, so that what's prepared ahead goes on from stage to stage.
        self.prepare_cells();
        let state = self.world.resource::<CellState>();
        let mut changed = false;
        if state.is_some_and(|state| !state.coming.is_empty()) && self.cells_ready() {
            self.enter_coming();
            changed = true;
        }
        let viewers = self.world.query::<(Entity, &cells::Viewer)>();
        let radius = viewers.fold(VIEWED_RADIUS, |most, (_, viewer)| most.max(viewer.radius));
        let viewer = (self.look.viewpoint).map(|seen| {
            let at = seen.position.to_array().map(f64::from);
            (at, f64::from(radius))
        });
        let listed = (self.cells).listed(self.root.as_deref(), &self.files, &folder);
        let made = self.cells.made(&folder).map(|made| (made.from, made.to));
        let mut plan = mem::take(&mut self.cells.plan);
        self.world
            .plan_cells(size, &listed, made, viewer, &mut plan);
        // The cells that streamed roots are in, out and neither coming nor refused, come in with
        // those near a viewer.
        let mut entering = plan.entering.clone();
        let state = self.world.resource::<CellState>();
        let waiting = |cell: &Cell| {
            state.is_some_and(|state| state.coming.contains(cell) || state.refused.contains(cell))
        };
        let strays = plan.strays.iter().map(|&(cell, _)| cell);
        entering.extend(strays.filter(|cell| !waiting(cell)));
        entering.sort_unstable();
        entering.dedup();
        self.world.note_coming(&entering);
        self.world.forget_refused(&plan.forgotten);
        changed |= !entering.is_empty() || !plan.forgotten.is_empty();
        let changes = self.scenes.changes;
        if self.cells.held.1 != changes {
            self.cells.held = (Vec::new(), changes);
        }
        for &cell in &plan.leaving {
            if self.cells.held.0.contains(&cell) {
                continue;
            }
            let started = Instant::now();
            let roots = roots_of(&plan.leavers, cell);
            let state = self.world.resource::<CellState>();
            let came = state.is_some_and(|state| state.files.contains_key(&cell));
            let source = self.cells.source(&folder, &listed, cell);
            let file = came.then(|| self.source_text(&folder, cell, source));
            let same = match file.transpose() {
                Ok(file) => matches!(self.world.cell_written(file.as_deref(), &roots), Ok(None)),
                Err(_) => false,
            };
            if !same {
                self.cells.held.0.push(cell);
                continue;
            }
            self.world.forget_cell(cell, &roots);
            changed = true;
            debug!(
                code = "cells.left",
                cell = %cell,
                roots = roots.len(),
                ms = perf::millis(started.elapsed()),
                "A cell went out as its file has it"
            );
        }
        let coming = |cell: &Cell, world: &World| {
            let state = world.resource::<CellState>();
            state.is_some_and(|state| state.coming.contains(cell))
        };
        let (preparing, world) = (&mut self.cells.preparing.cells, &self.world);
        preparing.retain(|cell, _| plan.nearby.binary_search(cell).is_ok() || coming(cell, world));
        let size = f64::from(size);
        let prepare = (plan.ahead.iter())
            .filter(|&&(past, cell)| past <= size && !preparing.contains_key(&cell))
            .map(|&(_, cell)| cell);
        let prepare: Vec<Cell> = prepare.collect();
        for cell in prepare {
            let source = self.cells.source(&folder, &listed, cell);
            self.start_preparing(&folder, cell, source);
        }
        self.cells.plan = plan;
        if changed {
            self.scenes.changed();
        }
    }

    /// The world's cells, as the tool API's `cells` reads them, the world as edited at `ticks`, as
    /// [`scene_files::unsaved_cells`] finds those a save would write; `stopped` whether the run is,
    /// as the look's viewpoint is a viewer then, as [`stream_stopped`](Self::stream_stopped) says.
    #[cfg(feature = "dev")]
    pub(crate) fn cells_read(&mut self, stopped: bool, ticks: u64) -> gg_api::CellList {
        let Some((id, cells, _)) = self.world.world_cells() else {
            return gg_api::CellList::default();
        };
        let (entity, folder, size) = (id.to_string(), cells.folder.clone(), cells.size);
        let mut viewers = Vec::new();
        let mut radius = VIEWED_RADIUS;
        for (entity, viewer) in self.world.query::<(Entity, &cells::Viewer)>() {
            radius = radius.max(viewer.radius);
            let at = self.world.pose(entity).expect("alive").matrix.translation;
            viewers.push(gg_api::ViewerAt {
                entity: self.world.id(entity).map(|id| id.to_string()),
                position: at.to_array(),
                radius: viewer.radius,
            });
        }
        if stopped
            && !self.client
            && let Some(seen) = self.look.viewpoint
        {
            viewers.push(gg_api::ViewerAt {
                entity: None,
                position: seen.position.to_array(),
                radius,
            });
        }
        let unsaved = scene_files::unsaved_cells(self, ticks).to_vec();
        let listed = (self.cells).listed(self.root.as_deref(), &self.files, &folder);
        let mut roots = Vec::new();
        if size.is_finite() && size > 0.0 {
            self.world.streamed_roots(size, &mut roots);
        }
        let empty = CellState::default();
        let state = self.world.resource::<CellState>().unwrap_or(&empty);
        let mut all: Vec<Cell> = (listed.iter())
            .chain(&state.cells_in)
            .chain(&state.coming)
            .chain(state.kept.keys())
            .chain(&state.refused)
            .copied()
            .chain(roots.iter().map(|&(cell, ..)| cell))
            .collect();
        all.sort_unstable();
        all.dedup();
        let made = self.cells.made(&folder);
        let cells = all.into_iter().map(|cell| {
            let from = roots.partition_point(|&(other, ..)| other < cell);
            let to = roots.partition_point(|&(other, ..)| other <= cell);
            let file = listed.binary_search(&cell).is_ok();
            gg_api::CellAt {
                cell: cell.0.to_array(),
                file,
                made: !file && made.is_some_and(|made| made.makes(cell)),
                is_in: state.cells_in.contains(&cell),
                coming: state.coming.contains(&cell),
                refused: state.refused.contains(&cell),
                kept: state.kept.get(&cell).map_or(0, |kept| kept.texts.len()),
                roots: to - from,
                unsaved: unsaved.binary_search(&cell).is_ok(),
            }
        });
        let world = gg_api::WorldCells {
            entity,
            folder,
            size,
            viewers,
            cells: cells.collect(),
        };
        gg_api::CellList { world: Some(world) }
    }

    /// Lists the world's cells' folder again, as a development build's look at its scene files
    /// does while the run can stop, so that a cell's file made since brings its cell in, and one
    /// taken away no longer does. A folder that can't be listed keeps the cells it had, its first
    /// listing having warned (`cells.listing`).
    #[cfg(feature = "dev")]
    pub(crate) fn list_cells_again(&mut self) {
        let Some((_, cells, _)) = self.world.world_cells() else {
            return;
        };
        let folder = cells.folder.as_str();
        let Ok(listed) = cells::list_cells(self.root.as_deref(), &self.files, folder) else {
            return;
        };
        match (self.cells.listed.iter_mut()).find(|(listed, _)| listed == folder) {
            Some((_, was)) => *was = listed,
            None => self.cells.listed.push((folder.to_owned(), listed)),
        }
    }

    /// Notes that each of `cells`' files, of its digest, holds what the world as edited holds of
    /// its cell, as a world's save leaves them, as [`World::note_filed`] says: of the world held
    /// aside while a copy plays, and else of the context's own, between ticks.
    #[cfg(feature = "dev")]
    pub(crate) fn note_filed(&mut self, cells: impl IntoIterator<Item = (Cell, u64)>) {
        let world = match &mut self.edited {
            Some(edited) => &mut edited.world,
            None => &mut self.world,
        };
        for (cell, digest) in cells {
            world.note_filed(cell, digest);
        }
    }

    /// Empties what the world holds of its cells and what the run prepared of them, as the tool
    /// API's `scene new` and `open` put another scene in a world's place, so that the next world's
    /// cells come in from their files.
    #[cfg(feature = "dev")]
    pub(crate) fn empty_cells(&mut self) {
        self.world.empty_cells();
        self.cells.preparing.cells.clear();
        self.cells.held = Default::default();
    }

    /// Brings in the cells the last tick's end found coming, as
    /// [`stream_cells`](Self::stream_cells) says, as a tick starts, before its layers', wherever a
    /// tick runs: each in order, its file read, unless what the cell keeps stands for it, as
    /// [`scene_text`](Self::scene_text) keeps it, once a run, and what it and the texts the cell
    /// keeps name loaded first, as [`load_assets_of`](Self::load_assets_of) does for a scene
    /// change, in a server's session none; each text parsed, and the cell's entities spawned. A
    /// world whose `Cells` went meanwhile drops them. A server's client brings none in.
    ///
    /// Logs, at the debug level, `cells.entered`, with the cell, how many roots came, whether it
    /// came as prepared ahead, as [`cells_ready`](Self::cells_ready) prepares it, and how long it
    /// took; and notes, for a recording, the digest of each cell's file that came in. A cell that
    /// doesn't come in, as one whose file gives an entity's ID that's alive (`ecs.id_taken`) or
    /// names a model that doesn't load, stays out, warning `cells.refused`, with why, until every
    /// viewer has gone.
    pub(crate) fn enter_coming(&mut self) {
        self.cells.came.clear();
        if self.client {
            return;
        }
        let coming = self.world.take_coming();
        if coming.is_empty() {
            return;
        }
        let Some((_, cells, _)) = self.world.world_cells() else {
            return;
        };
        let folder = cells.folder.clone();
        let listed = (self.cells).listed(self.root.as_deref(), &self.files, &folder);
        for cell in coming {
            let started = Instant::now();
            let source = self.cells.source(&folder, &listed, cell);
            let filed = matches!(source, Some(CellSource::File));
            match self.enter_cell(&folder, cell, source) {
                Ok((roots, file, prepared)) => {
                    // A text the game made isn't a file's, which a recording would name.
                    let file = file.filter(|_| filed);
                    self.cells.came.extend(file.map(|digest| (cell, digest)));
                    debug!(
                        code = "cells.entered",
                        cell = %cell,
                        roots,
                        prepared,
                        ms = perf::millis(started.elapsed()),
                        "A cell came in"
                    );
                }
                Err(error) => {
                    if self.cells.first("cells.refused", cell) {
                        warn!(
                            code = "cells.refused",
                            cell = %cell,
                            refused = error.code(),
                            "{error:#}; the cell stays out until every viewer has gone"
                        );
                    }
                    self.world.refuse_cell(cell);
                }
            }
        }
    }

    /// Brings cell `cell`, of the cells of `folder`, in, as
    /// [`enter_coming`](Self::enter_coming) says: its file read, or the game's making of it, as
    /// `source` says, and what it names loaded, unless what the cell keeps stands for it, and what
    /// each text it keeps names; or as prepared ahead, if what was prepared stands for its texts
    /// still, as [`stands_for`](Self::stands_for) says. Returns how many roots came, the digest of
    /// its file's text, if it came, and whether it came as prepared.
    ///
    /// Fails, bringing nothing in, as [`scene_text`](Self::scene_text) does, as
    /// [`load_assets_of`](Self::load_assets_of) does, and as [`World::parse_scene`] and
    /// [`World::enter_cell`] do, the cell's path beginning the message.
    fn enter_cell(
        &mut self,
        folder: &str,
        cell: Cell,
        source: Option<CellSource>,
    ) -> Result<(usize, Option<u64>, bool)> {
        let path = format!("{folder}/{cell}.ron");
        let prepared = match self.cells.preparing.cells.remove(&cell) {
            Some(Prep {
                stage: Stage::Ready(Ok(prepared)),
                ..
            }) => Some(prepared),
            _ => None,
        };
        let state = self.world.resource::<CellState>();
        let came = state.is_some_and(|state| state.files.contains_key(&cell));
        let kept = state.and_then(|state| state.kept.get(&cell)).cloned();
        let kept = kept.unwrap_or_default();
        let named = |error: Error| {
            let message = format!("{path}:{error}");
            error.with_message(message)
        };
        let source = source.filter(|_| !came);
        if let Some(prepared) = prepared
            && self.stands_for(&prepared, &path, source.as_ref(), &kept)
        {
            let file = prepared.file;
            let roots = self
                .world
                .enter_cell(cell, prepared.entering)
                .map_err(named)?;
            return Ok((roots, file, true));
        }
        let file = match source {
            Some(source) => Some(self.source_text(folder, cell, Some(source))?),
            None => None,
        };
        let texts = file.iter().map(String::as_str);
        for text in texts.chain(kept.texts.iter().map(|kept| &*kept.text)) {
            self.load_assets_of(&path, text)?;
        }
        let parse = |text: &str| self.world.parse_scene(text).map_err(named);
        let came_as = match &file {
            Some(text) if kept.texts.is_empty() => cells::came_as(&self.world, text),
            _ => None,
        };
        let file = file
            .as_deref()
            .map(|text| Ok((text_digest(text), parse(text)?)));
        let entering = Entering {
            file: file.transpose()?,
            kept: kept
                .texts
                .iter()
                .map(|kept| parse(&kept.text))
                .collect::<Result<_>>()?,
            came_as,
        };
        let file = entering.file.as_ref().map(|&(digest, _)| digest);
        let roots = self.world.enter_cell(cell, entering).map_err(named)?;
        Ok((roots, file, false))
    }

    /// Whether `prepared`, cell `path`'s, was parsed from the texts the cell would bring in now:
    /// what its `source` gives, its file's, as [`scene_text`](Self::scene_text) gives it, or the
    /// game's making of it, for a cell whose file is yet to come in; and those it keeps, `kept`. A
    /// development build's save of the file since, a file made for a made cell, or a root left in
    /// the cell since, makes it stand for them no more.
    fn stands_for(
        &mut self,
        prepared: &Prepared,
        path: &str,
        source: Option<&CellSource>,
        kept: &Kept,
    ) -> bool {
        let kept = kept.texts.iter().map(|kept| kept.digest);
        if !prepared.kept.iter().copied().eq(kept)
            || prepared.file.is_some() != source.is_some()
            || prepared.made != matches!(source, Some(CellSource::Made(_)))
        {
            return false;
        }
        match (prepared.file, source) {
            (Some(digest), Some(CellSource::File)) => {
                (self.scene_text(path)).is_ok_and(|text| text_digest(&text) == digest)
            }
            // The game makes a cell the same each time.
            _ => true,
        }
    }

    /// The text cell `cell` of the cells of `folder` comes in from, as `source` says: its file's, as
    /// [`scene_text`](Self::scene_text) gives it, or what the game makes of it, unless the run
    /// keeps a file of it, as a development build's save of a made cell that changed writes one.
    ///
    /// Fails as `scene_text` does, and with `scene.not_found` for a cell of neither.
    pub(crate) fn source_text(
        &mut self,
        folder: &str,
        cell: Cell,
        source: Option<CellSource>,
    ) -> Result<String> {
        let path = format!("{folder}/{cell}.ron");
        let cached = self.scenes.cached.iter().find(|file| file.author == path);
        match source {
            Some(CellSource::File) => self.scene_text(&path),
            Some(CellSource::Made(_)) if let Some(file) = cached => Ok(file.text.clone()),
            Some(CellSource::Made(made)) => Ok((made.text)(cell)),
            None => {
                let message = format!("there's no scene {path}, nor does the game make the cell");
                Err(Error::new("scene.not_found", message))
            }
        }
    }

    /// Starts preparing cell `cell`, of the cells of `folder`, as [`Preparing`] says: on a thread,
    /// its file read, as the run keeps it or from the disk or the game's package, or the game's
    /// making of it, as `source` says, if it's yet to come in, and the texts it keeps taken with
    /// it.
    fn start_preparing(&mut self, folder: &str, cell: Cell, source: Option<CellSource>) {
        let path = format!("{folder}/{cell}.ron");
        let state = self.world.resource::<CellState>();
        let came = state.is_some_and(|state| state.files.contains_key(&cell));
        let kept = (state.and_then(|state| state.kept.get(&cell)))
            .map(|kept| {
                let texts = kept.texts.iter();
                texts
                    .map(|kept| (Arc::clone(&kept.text), kept.digest))
                    .collect()
            })
            .unwrap_or_default();
        let source = source.filter(|_| !came);
        let made = matches!(source, Some(CellSource::Made(_)));
        let read = source.map(|source| {
            let cached = self.scenes.cached.iter().find(|file| file.author == path);
            let cached: Option<Arc<str>> = cached.map(|file| file.text.as_str().into());
            let (root, files, scene) = (self.root.clone(), self.files.clone(), path.clone());
            move || {
                if let Some(text) = cached {
                    return Ok(text);
                }
                if let CellSource::Made(made) = source {
                    return Ok((made.text)(cell).into());
                }
                let root = root.ok_or_else(|| no_root(&scene))?;
                let at = package::under(&root, &scene);
                Self::read_scene_text(&files, &root, &scene, &at, true).map(Arc::from)
            }
        });
        let world = self.world.scratch();
        let number = self.cells.preparing.begin(cell);
        (self.cells.preparing).read((cell, number), (path, made), read, kept, world);
    }

    /// Takes what the threads preparing cells have finished, and starts the next stage of each:
    /// for a cell whose texts name what isn't loaded, its models' files read on threads, then the
    /// rest loaded here, as [`load_assets_of`](Self::load_assets_of) loads it for a scene change,
    /// none in a server's session, and its texts parsed on a thread. A cell prepared, or that
    /// couldn't be, logs `cells.prepared` at the debug level, with how long it took and how long
    /// the main thread spent on it. A file read is kept as [`scene_text`](Self::scene_text) keeps
    /// it, unless the run has it already.
    pub(crate) fn prepare_cells(&mut self) {
        while let Some(done) = self.cells.preparing.take() {
            let started = Instant::now();
            let ready = match done {
                Finished::Read {
                    cell,
                    number,
                    texts,
                    missing,
                    parsed,
                } => {
                    if self.cells.preparing.prep(cell, number).is_none() {
                        continue;
                    }
                    // A text the game made it makes again.
                    if let Ok(Texts {
                        path,
                        file: Some((text, _)),
                        made: false,
                        ..
                    }) = &texts
                        && let Some(root) = &self.root
                        && !self.scenes.cached.iter().any(|file| file.author == *path)
                    {
                        let at = package::under(root, path);
                        let file = SceneFile::new(at, path.clone(), text.to_string());
                        self.scenes.cache(file);
                    }
                    let stage = match (texts, parsed) {
                        (Err(error), _) => Stage::Ready(Err(error)),
                        (Ok(_), Some(parsed)) => Stage::Ready(parsed),
                        (Ok(texts), None) => self.load_for((cell, number), texts, &missing),
                    };
                    vec![(cell, number, stage)]
                }
                Finished::Model { path, read } => {
                    self.cells.preparing.model_read(path.clone(), read);
                    let mut loaded = Vec::new();
                    for (&cell, prep) in &mut self.cells.preparing.cells {
                        if let Stage::Loading { models, .. } = &mut prep.stage {
                            models.retain(|model| *model != path);
                            if models.is_empty() {
                                loaded.push(cell);
                            }
                        }
                    }
                    let mut stages = Vec::new();
                    for cell in loaded {
                        let prep = self.cells.preparing.cells.get_mut(&cell).expect("loading");
                        let Stage::Loading { texts, .. } =
                            mem::replace(&mut prep.stage, Stage::Parsing)
                        else {
                            unreachable!("loading, as found above");
                        };
                        let number = prep.number();
                        let stage = self.finish_loading((cell, number), texts);
                        stages.push((cell, number, stage));
                    }
                    stages
                }
                Finished::Parsed {
                    cell,
                    number,
                    parsed,
                } => vec![(cell, number, Stage::Ready(parsed))],
            };
            let took = started.elapsed();
            for (cell, number, stage) in ready {
                let Some(prep) = self.cells.preparing.prep(cell, number) else {
                    continue;
                };
                prep.main += took;
                prep.stage = stage;
                if let Stage::Ready(prepared) = &prep.stage {
                    debug!(
                        code = "cells.prepared",
                        cell = %cell,
                        refused = prepared.as_ref().err().map(Error::code),
                        ms = perf::millis(prep.started.elapsed()),
                        main_ms = perf::millis(prep.main),
                        "A cell was prepared to come in"
                    );
                }
            }
        }
        self.cells.preparing.forget_models();
    }

    /// The stage after cell `cell`'s texts, `texts`, were found to name `missing`, which isn't
    /// loaded, for its preparing numbered `number`: the files of the models among them read on
    /// threads, which it waits for, unless none is to read, as in a server's session, which loads
    /// none; then, as [`finish_loading`](Self::finish_loading) says.
    fn load_for(
        &mut self,
        (cell, number): (Cell, u64),
        texts: Texts,
        missing: &[Asset<String>],
    ) -> Stage {
        let mut models = Vec::new();
        for asset in missing {
            let Asset::Model(name) = asset else {
                continue;
            };
            // What fails here, `load_assets_of` says as it loads them.
            let Ok(path) = self.asset_path(name, "model", false) else {
                continue;
            };
            let file = match &self.root {
                Some(root) => root.join(&*path),
                None => PathBuf::from(&*path),
            };
            if self.served
                || self.assets.has(&asset.as_deref())
                || self.within_root(&path, &file).is_err()
            {
                continue;
            }
            (self.cells.preparing).read_model(path, file.clone(), self.files.clone());
            if self.cells.preparing.reading(&file) {
                models.push(file);
            }
        }
        match models.is_empty() {
            true => self.finish_loading((cell, number), texts),
            false => Stage::Loading { texts, models },
        }
    }

    /// The stage after what cell `cell`'s texts, `texts`, name has been read, for its preparing
    /// numbered `number`: what they name loaded, as [`load_assets_of`](Self::load_assets_of) loads
    /// it, the models' files read on threads taken; and then its texts parsed on a thread, in a
    /// scratch world that has what was loaded; or, ready, why it couldn't be loaded.
    fn finish_loading(&mut self, (cell, number): (Cell, u64), texts: Texts) -> Stage {
        for text in texts.each() {
            if let Err(error) = self.load_assets_of(&texts.path, text) {
                return Stage::Ready(Err(error));
            }
        }
        self.share_assets();
        let world = self.world.scratch();
        self.cells.preparing.parse((cell, number), texts, world);
        Stage::Parsing
    }

    /// Whether the cells coming in as the next tick starts are prepared, as
    /// [`enter_coming`](Self::enter_coming) brings them in: taking what the threads have finished,
    /// as [`prepare_cells`](Self::prepare_cells) does, whatever's coming, and starting to prepare
    /// those coming that aren't being prepared; true for a server's client, which
    /// brings none in, and in a world with none coming. A tick that isn't run, as it waits for
    /// them, starts a wait, which logs `cells.waited` at the debug level as it ends, with how long it
    /// took; in a context without threads, as a test's, each is prepared then and there.
    pub(crate) fn cells_ready(&mut self) -> bool {
        if self.client {
            return true;
        }
        // Every tick, so that what's prepared ahead goes on from stage to stage between entries.
        self.prepare_cells();
        let state = self.world.resource::<CellState>();
        let Some(coming) = state.map(|state| &state.coming).filter(|c| !c.is_empty()) else {
            return true;
        };
        let coming: Vec<Cell> = coming.iter().copied().collect();
        let unprepared = |cx: &Self| {
            let cells = &cx.cells.preparing.cells;
            coming
                .iter()
                .copied()
                .filter(|cell| !cells.contains_key(cell))
                .collect::<Vec<_>>()
        };
        let unprepared = unprepared(self);
        if !unprepared.is_empty() {
            let Some((_, cells, _)) = self.world.world_cells() else {
                return true;
            };
            let folder = cells.folder.clone();
            let listed = (self.cells).listed(self.root.as_deref(), &self.files, &folder);
            for cell in unprepared {
                let source = self.cells.source(&folder, &listed, cell);
                self.start_preparing(&folder, cell, source);
            }
            self.prepare_cells();
        }
        let preparing = &mut self.cells.preparing;
        let ready = (coming.iter()).all(|cell| {
            let prep = preparing.cells.get(cell);
            matches!(prep.map(|prep| &prep.stage), Some(Stage::Ready(_)))
        });
        match (ready, preparing.waiting) {
            (true, Some(since)) => {
                preparing.waiting = None;
                debug!(
                    code = "cells.waited",
                    cells = coming.len(),
                    ms = perf::millis(since.elapsed()),
                    "A tick waited for the cells it brings in to be prepared"
                );
            }
            (false, None) => preparing.waiting = Some(Instant::now()),
            _ => {}
        }
        ready
    }

    /// Warns, once a run, that cell `cell`'s roots couldn't be kept as text, with `error`.
    fn held(&mut self, cell: Cell, error: &Error) {
        if self.cells.first("cells.held", cell) {
            warn!(
                code = "cells.held",
                cell = %cell,
                held = error.code(),
                "{error:#}; the cell's streamed roots stay in the world"
            );
        }
    }

    /// Loads what scene `text` names that no layer has added, for a scene change, as
    /// [`change_scene`](Self::change_scene) says: its models and prefabs first, and those the
    /// prefabs name, in turn, none standing in, so that a refusal leaves no stand-in behind; and
    /// then its textures and fonts, standing in for those that can't be loaded.
    ///
    /// Fails as [`World::load_scene`] does into the world without its entities, but for what it
    /// names, and with `asset.no_model` or `asset.no_prefab`, saying why, for a model or a prefab
    /// that doesn't load.
    fn load_change_assets(&mut self, text: &str) -> Result<()> {
        loop {
            let names = self.world.scene_assets(text)?;
            let heavy =
                |asset: &&Asset<String>| matches!(asset, Asset::Model(_) | Asset::Prefab(_));
            let heavy: Vec<&Asset<String>> = names.iter().filter(heavy).collect();
            if heavy.is_empty() {
                break;
            }
            self.load_named(heavy.iter().map(|asset| asset.as_deref()), false);
            if let Some(asset) = heavy.into_iter().find(|asset| !self.assets.has(asset)) {
                let (code, kind) = match asset {
                    Asset::Model(_) => ("asset.no_model", "model"),
                    _ => ("asset.no_prefab", "prefab"),
                };
                let message = format!(
                    "{}; a scene change stands in for no model or prefab, which would leave the \
                     scene without what they hold",
                    self.assets.not_found(kind, asset.name())
                );
                return Err(Error::new(code, message));
            }
        }
        self.load_scene_assets_as(text, true).map(drop)
    }

    /// The text of the scene file `scene` names, its path from the asset root, for the scene
    /// changes of the run: as the run read it once, as it started or as a change first named it,
    /// or as a development build's edit took a save of it since, so that a tick run again, a
    /// replay and each lockstep peer read the same, as [`World::change_scene`] says; and else read
    /// now, from the game's package in a run that reads one and from the disk otherwise, and kept.
    ///
    /// Fails with `scene.no_root` in a run without an asset root, as [`App::root`] gives one;
    /// `scene.not_found`, naming it, for a file that isn't there; `scene.case` for one whose name
    /// is the file's but in its case, which a package or another OS's disk wouldn't find;
    /// `asset.outside_root` for a link that leads outside the root; and `scene.file` for one that
    /// can't be read, is more than 16 MiB or isn't UTF-8.
    ///
    /// [`App::root`]: crate::App::root
    pub(crate) fn scene_text(&mut self, scene: &str) -> Result<String> {
        if let Some(file) = self.scenes.cached.iter().find(|file| file.author == scene) {
            return Ok(file.text.clone());
        }
        let Some(root) = self.root.clone() else {
            return Err(no_root(scene));
        };
        let path = package::under(&root, scene);
        let text = Self::read_scene_text(&self.files, &root, scene, &path, true)?;
        self.scenes
            .cache(SceneFile::new(path, scene.to_owned(), text.clone()));
        Ok(text)
    }

    /// Reads the scene file `scene` names, at `path` under `root`, from `files`, as
    /// [`scene_text`](Self::scene_text) does, on any thread: checking its name against the disk's,
    /// part by part, if `named`, for one the disk didn't give.
    pub(crate) fn read_scene_text(
        files: &Files,
        root: &Path,
        scene: &str,
        path: &Path,
        named: bool,
    ) -> Result<String> {
        /// The most a scene file holds, as the most a prefab's does.
        const MOST: u64 = 16 << 20;
        let packaged = files.packaged().is_some();
        let not_found = |within: &str| {
            let message = format!("there's no scene {scene} in {within}");
            Error::new("scene.not_found", message)
        };
        let case = |found: &str| {
            let message = format!(
                "the scene {scene} is {found}, in another case, which a package or a disk that \
                 tells case apart, as Linux's does, wouldn't find: name it as its file is named"
            );
            Error::new("scene.case", message)
        };
        if !packaged {
            let exactly = if named {
                scene_files::named_exactly(root, scene)
            } else {
                Ok(())
            };
            exactly.map_err(|found| {
                found.map_or_else(|| not_found("the asset root"), |found| case(&found))
            })?;
            let canonical = |path: &Path| std::fs::canonicalize(path).ok();
            if let (Some(root), Some(file)) = (canonical(root), canonical(path))
                && !file.starts_with(root)
            {
                let message = format!(
                    "the scene {scene} is a link to {}, outside the asset root, which a package \
                     wouldn't hold",
                    file.display()
                );
                return Err(Error::new("asset.outside_root", message));
            }
        }
        let bytes = match files.read(path, MOST + 1) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && packaged => {
                let paths = files.paths();
                let other = (paths.iter())
                    .filter_map(|other| other.strip_prefix(root).ok())
                    .map(|other| other.to_string_lossy().replace('\\', "/"))
                    .find(|other| other.to_lowercase() == scene.to_lowercase());
                return Err(match other {
                    Some(other) => case(&other),
                    None => not_found("the game's package").with_source(error),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(not_found("the asset root").with_source(error));
            }
            Err(error) => {
                let message = format!("can't read the scene {scene}");
                return Err(Error::new("scene.file", message).with_source(error));
            }
        };
        if bytes.len() as u64 > MOST {
            let message = format!("the scene {scene} is more than 16 MiB, which no change reads");
            return Err(Error::new("scene.file", message));
        }
        String::from_utf8(bytes).map_err(|error| {
            let message = format!("the scene {scene} isn't UTF-8 text");
            Error::new("scene.file", message).with_source(error)
        })
    }

    /// Reads each scene file in the project's scenes folders, in a development build reading the
    /// disk, and keeps its text for the scene changes of the run, as
    /// [`scene_text`](Self::scene_text) keeps one: as the run starts, so that each lockstep peer,
    /// and a replay, reads them as it starts, as it reads its start scene, rather than at a tick.
    /// One that can't be read is left for a change to name, which fails as `scene_text` does.
    #[cfg(feature = "dev")]
    pub(crate) fn read_scenes(&mut self) {
        let _zone = gg_core::zone!();
        let Some(root) = self.root.clone() else {
            return;
        };
        if self.reads_package() {
            return;
        }
        let mut found = Vec::new();
        // Each folder as the disk names it, its files' names the disk's, so that each directory
        // is listed once.
        let named = |folder: &PathBuf| {
            let name = assets::name_of(Some(&root), folder);
            package::inside(&name) && scene_files::named_exactly(&root, &name).is_ok()
        };
        let mut folders: Vec<PathBuf> = self.folders.iter().filter(|f| named(f)).cloned().collect();
        while let Some(folder) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() && !hidden => folders.push(path),
                    Ok(kind)
                        if kind.is_file()
                            && !hidden
                            && path.extension().is_some_and(|ext| ext == "ron") =>
                    {
                        found.push(path);
                    }
                    _ => {}
                }
            }
        }
        found.sort();
        for path in found {
            let scene = assets::name_of(Some(&root), &path).to_string();
            let cached = self.scenes.cached.iter().any(|file| file.author == scene);
            if cached || !package::inside(&scene) {
                continue;
            }
            if let Ok(text) = Self::read_scene_text(&self.files, &root, &scene, &path, false) {
                self.scenes.cache(SceneFile::new(path, scene, text));
            }
        }
    }

    /// Once a tick is final, as it is at once in a run alone, a replay, a lockstep session that
    /// predicts nothing and a server, and in a rollback session once it has run on both peers'
    /// inputs, and as such a tick starts, the world before it final too: takes what changed the
    /// world's scene since it last did, as [`World::change_scene`] says, logging `scene.replaced`;
    /// the scene's file is then the scene file in the world, which a development build applies the
    /// saves of, in place of those the layers loaded, whose entities went, and in a project's game
    /// the open scene. And logs the last change refused since, at the error level, with its code
    /// and how many were. A client's ticks are never final, its server's world the one that is.
    pub(crate) fn confirm_scene(&mut self) {
        self.confirm_scene_from(None);
    }

    /// Takes the world's scene, and the scene changes refused, as what the run keeps of them
    /// already, as [`confirm_scene`](Self::confirm_scene) would: as a world comes in from a
    /// snapshot, a lockstep host, or held aside while a copy played, with its scene files.
    pub(crate) fn seen_scene(&mut self) {
        self.scene_seen = self.world.scene_changes();
        self.refused_seen = self.world.scenes_refused();
    }

    /// As [`confirm_scene`](Self::confirm_scene) does, of `world`, a world that's final, or of the
    /// context's own, for `None`: allocating nothing while nothing changed.
    pub(crate) fn confirm_scene_from(&mut self, world: Option<&World>) {
        let world = match world {
            Some(world) => world,
            None => &self.world,
        };
        let (changes, refused) = (world.scene_changes(), world.scenes_refused());
        if (changes, refused) == (self.scene_seen, self.refused_seen) {
            return;
        }
        let changed = changes != self.scene_seen;
        let scene = world.scene().filter(|_| changed).map(str::to_owned);
        let last = (refused != self.refused_seen)
            .then(|| world.resource::<crate::scene_change::Refused>())
            .flatten()
            .and_then(|refused| refused.last.clone());
        let times = refused - self.refused_seen;
        (self.scene_seen, self.refused_seen) = (changes, refused);
        if let Some((code, refused, message)) = last {
            error!(
                code = &*code,
                scene = &*refused,
                times,
                "{message}; the world stays as it was"
            );
        }
        let Some(scene) = scene else {
            return;
        };
        info!(
            code = "scene.replaced",
            scene = &*scene,
            changes,
            "The scene's entities took the place of the world's"
        );
        let file = (self.scenes.cached.iter())
            .find(|file| file.author == scene)
            .cloned();
        self.scenes.files.clear();
        self.scenes.open = scene_files::Open {
            author: (self.start.is_some() && file.is_some()).then(|| scene.clone()),
            ..scene_files::Open::default()
        };
        self.scenes.files.extend(file);
        #[cfg(feature = "dev")]
        self.scenes.changed();
    }

    /// The paths of the files the run's game's package holds, as
    /// [`App::from_package`](crate::App::from_package) and a project's game built without `dev`
    /// read one, each joined to the asset root as layers name files, which
    /// [`Scripts::load`](crate::Scripts::load), [`load_scene_file`](Self::load_scene_file) and the
    /// adds, such as [`add_texture`](Self::add_texture), take, in the order of the paths' bytes;
    /// none for a run that reads the disk. A player loads a game's scripts by them, as the
    /// reference player does, which runs a game without its code.
    pub fn package_files(&self) -> Vec<PathBuf> {
        self.files.paths()
    }

    /// Applies `edit`, an author's from outside the layers, to the world, between ticks, unrecorded,
    /// as a replay does: [`Stack::edit`] records it first. A script's reload, which no author
    /// undoes, goes to the [`Scripts`](crate::Scripts) that loaded it.
    ///
    /// Fails as [`Authors::edit`] does, and as [`Hosts::reload`](crate::luau::Hosts::reload) does.
    pub(crate) fn apply_edit(&mut self, edit: Edit) -> Result<Done> {
        #[cfg(feature = "dev")]
        self.scenes.changed();
        match edit {
            Edit::Script { path, text } => {
                self.hosts.reload(&mut self.world, &path, text)?;
                Ok(Done::Applied(Vec::new()))
            }
            Edit::Scene { path, text } => {
                // A cell in came from another text, which it no longer goes out as it came from.
                if let Some(cell) = self.world.cell_of_file(&path) {
                    self.world.forget_came_as(cell);
                }
                let root = self.root.clone().unwrap_or_default();
                let file = SceneFile::new(package::under(&root, &path), path, text);
                self.scenes.cache(file);
                Ok(Done::Applied(Vec::new()))
            }
            // Every log goes, the authors' and those of layers, such as the Inspector's, which the
            // world renewed refuses, freeing nothing twice.
            Edit::Renew => {
                let dropped = self.authors.entries(&self.world);
                self.authors.clear(&mut self.world);
                self.world.renew();
                if dropped > 0 {
                    warn!(
                        code = "edits.history_cleared",
                        dropped,
                        "Dropped the {dropped} edits the authors could undo or redo, as a lockstep \
                         joiner joined, or the peers' worlds were made the same again: no log of \
                         the joiner's holds them"
                    );
                }
                Ok(Done::Applied(Vec::new()))
            }
            edit => {
                let done = self.authors.edit(&mut self.world, edit)?;
                // Of the world in the context's place: a copy's goes with it at `stop`.
                self.scenes.open.edited = true;
                Ok(done)
            }
        }
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
    /// them on from; and moves the world's change tick on, so that what the tick changes is marked
    /// as changed since it started.
    pub(crate) fn capture(&mut self) {
        self.past.capture(&mut self.world);
        self.tick_started = self.world.change_tick();
    }

    /// Draws the world's entities, after what the layers drew this frame, each `alpha` of the way
    /// from where it was as the latest of the `ticks` ticks run started to where the tick left it,
    /// at `time`, the simulation's in seconds, emitters' particles too, and their colliders'
    /// outlines, over the rest, while they're shown, or the look shows them; and finds where the
    /// world's camera sees from, the world's environment, and its lights, where they draw. Before
    /// the entities, a development build draws the look's grid, unless the frame is bare, as the
    /// viewpoint the frame sees from so far sees it.
    pub(crate) fn draw_world(&mut self, alpha: f32, time: f32, ticks: u64) {
        self.past.resolve(&self.world);
        self.placed.place(&self.world);
        let held = self.world.component::<MeshDraw>();
        (self.placed).draw(&self.world, &self.past, alpha, self.tick_started, held);
        let (made, meshes, draws) = (&self.loaded.made, &mut self.meshes, &mut self.draws);
        (self.skins).find(&self.world, &self.placed, made, meshes, draws);
        let origin = self.origin;
        (self.opaque).hold(
            &self.world,
            &self.placed,
            &self.skins,
            &mut self.scene,
            origin,
        );
        let (world, past, placed) = (&self.world, &self.past, &self.placed);
        self.seen = camera::seen(world, past, placed, alpha, &mut self.tie);
        self.environment = environment::seen(world, &mut self.environment_tie);
        (self.lit).find(world, past, placed, (alpha, self.origin), self.tick_started);
        self.time = time;
        #[cfg(feature = "dev")]
        if let Some(plane) = self.look.grid.filter(|_| !self.look.bare) {
            let seen = self.seen.map(|(_, seen)| seen);
            let viewpoint = self.look.viewpoint.or(self.viewpoint).or(seen);
            self.draw_grid(&viewpoint.unwrap_or_default(), plane);
            self.draw_cells();
        }
        let (world, past, placed) = (&self.world, &self.past, &self.placed);
        let (draws, shaders, fonts) = (&mut self.draws, &mut self.shaders, &mut self.fonts);
        let scratch = &mut self.scratch;
        let drawn = (alpha, self.origin);
        draw::draw_world(
            world, past, placed, drawn, ticks, draws, shaders, fonts, scratch,
        );
        let (colliders, navmesh) = (self.colliders_drawn(), self.navmesh_drawn());
        if (colliders || navmesh)
            && let Some(poses) = Poses::new(world, past, placed, alpha, self.origin)
        {
            let scale = self.scale() as f32;
            let (over, shaders) = (&mut self.over, &mut self.shaders);
            if colliders {
                physics::draw_colliders(world, &poses, scale, over, shaders);
            }
            if navmesh {
                navigation::draw_navigation(world, &poses, scale, over, shaders);
            }
        }
        // What changes after, until the next frame draws, is marked past what this one saw.
        self.world.advance();
    }

    /// Draws the cubes of the world's cells in, each its twelve edges, as lines are drawn, in
    /// [`CELL_IN`], and of those refused in [`CELL_REFUSED`], as the look's grid draws, so that the
    /// editor's view shows where the cells are.
    #[cfg(feature = "dev")]
    fn draw_cells(&mut self) {
        let Some((_, cells, _)) = self.world.world_cells() else {
            return;
        };
        let size = cells.size;
        let state = self.world.resource::<CellState>();
        let Some(state) = state.filter(|_| size.is_finite() && size > 0.0) else {
            return;
        };
        let cells_in = state.cells_in.iter().map(|&cell| (cell, CELL_IN));
        let refused = state.refused.iter().map(|&cell| (cell, CELL_REFUSED));
        for (cell, color) in cells_in.chain(refused) {
            let low = (cell.0.as_dvec3() * f64::from(size) - self.origin).as_vec3();
            // Along each axis, the four edges from the corners low on it.
            for (axis, corner) in (0..3).flat_map(|axis| (0..4).map(move |corner| (axis, corner))) {
                let mut from = low;
                let across = [(axis + 1) % 3, (axis + 2) % 3];
                for (bit, across) in across.into_iter().enumerate() {
                    if corner & (1 << bit) != 0 {
                        from[across] += size;
                    }
                }
                let mut to = from;
                to[axis] += size;
                let line = Line {
                    from,
                    to,
                    color,
                    width: 1.5,
                };
                (self.over).push_line(&mut self.shaders, Mat4::IDENTITY, line, 0);
            }
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

    /// Whether the frame draws the navmesh and the agents' paths, as the layers set.
    fn navmesh_drawn(&self) -> bool {
        self.navmesh
    }

    /// Fills in `frame` with this frame's render data, with the game's view at `viewport`, which
    /// [`view`](Self::view) gives from then on: the world's environment's tonemapper,
    /// antialiasing, exposure and its adaptation, sky, ambient occlusion, fog, reflections and
    /// camera's effects, or none, none, the default's and none, none, none, none, none and none,
    /// the antialiasing a layer set in place of its own, and its lights, as the world was last
    /// drawn, and the time it was drawn at;
    /// its draws, those over the rest last, and its canvas's, placed from their anchors in the view's
    /// pixels, for the last frame's, which it forgets, keeping their memory for the next; the
    /// viewpoint a layer set this
    /// frame, which it forgets too, or else where the world's camera sees from, or else the
    /// default, which hears the sounds playing from a place, as [`Place`] says, and whether the
    /// view is cut to it, as it comes from elsewhere than the last frame's, as [`Seer`] says; the
    /// origin the frame drew from, which then follows that viewpoint, as [`origin`](Self::origin)
    /// says; what a layer
    /// outlined this frame, which it forgets too; the pixels the view shows,
    /// if it shows any, swapping new ones for those `frame` had; what's been added since a frame
    /// last took it, after what `frame` still holds, the images the loader has decoded since among
    /// it, and how many it's still decoding; and the picks asked this frame, each at
    /// the pixel of the view under it, the floor of where it was asked less the view's origin, but
    /// those outside the view, which the next answers to come back answer `None`. In a development
    /// build, the look's viewpoint and outline go in place of the layers', the passes it leaves
    /// out are left out, the frame carries the look's revision, and whether it's seen from the
    /// look's viewpoint, whose exposure adapts apart from the game's own; but a bare frame leaves
    /// the look out, and the picks to the next
    /// frame that isn't bare, both those asked this frame and those the renderer holds, so that
    /// none reads a view a client isn't shown.
    pub(crate) fn extract(&mut self, frame: &mut Extracted, viewport: Viewport) {
        frame.clear_color = self.clear_color;
        // A world without an environment isn't tonemapped or antialiased, but as a layer sets, and
        // its light is exposed as the default environment's is.
        frame.tonemapper = (self.environment).map_or(Tonemapper::None, |seen| seen.tonemapper);
        frame.antialiasing = self.antialiasing.unwrap_or_else(|| {
            (self.environment).map_or(Antialiasing::None, |seen| seen.antialiasing)
        });
        frame.exposure = self.environment.unwrap_or_default().light_scale();
        frame.adaptation = (self.environment)
            .and_then(|seen| seen.adaptation)
            .map(|adaptation| adaptation.rendered());
        frame.sky = self.environment.and_then(|seen| seen.sky());
        // Nor bloomed or graded.
        frame.bloom = self.environment.map_or(0.0, |seen| seen.bloom);
        frame.grade = self.environment.map_or(Grade::IDENTITY, |seen| seen.grade);
        frame.ambient_occlusion = self.environment.and_then(|seen| seen.ambient_occlusion);
        let fog = self.environment.and_then(|seen| seen.fog);
        let broken = fog.and_then(|fog| Some((fog, fog.non_finite()?)));
        if let Some((fog, field)) = broken
            && !mem::replace(&mut self.fog_warned, true)
        {
            warn!(
                code = "engine.bad_fog",
                field,
                "the world's fog has a {field} that isn't finite, {fog:?}: the view draws without \
                 fog"
            );
        }
        // The next streak of broken fog warns anew.
        self.fog_warned &= broken.is_some();
        frame.fog = fog.filter(|_| broken.is_none());
        frame.reflections = self.environment.and_then(|seen| seen.reflections);
        frame.atmosphere = self.environment.and_then(|seen| seen.atmosphere());
        frame.depth_of_field = (self.environment)
            .and_then(|seen| seen.depth_of_field)
            .map(|focus| focus.rendered());
        frame.motion_blur = (self.environment)
            .and_then(|seen| seen.motion_blur)
            .map(|blur| blur.rendered());
        frame.lens = self
            .environment
            .map_or(gg_renderer::Lens::NONE, |seen| seen.lens.rendered());
        frame.lights.clear();
        frame.lights.extend_from_slice(self.lit.lights());
        frame.time = self.time;
        frame.viewport = viewport;
        self.view = (viewport.size[0] > 0 && viewport.size[1] > 0).then_some(viewport);
        let (viewpoint, outline) = (self.viewpoint.take(), self.outline.take());
        let seen = match (viewpoint, self.seen) {
            (Some(viewpoint), _) => (Seer::Layer, viewpoint),
            (None, Some((camera, viewpoint))) => (Seer::Camera(camera), viewpoint),
            (None, None) => (Seer::Default, Viewpoint::default()),
        };
        let layers = outline.map(|(color, width)| (&self.outline_root[..], color, width));
        #[cfg(feature = "dev")]
        let (seen, outline) = if self.look.bare {
            (seen, layers)
        } else {
            let looked = (self.look.viewpoint).map(|looked| (Seer::Look(looked), looked));
            let wanted = (self.look.outline.as_ref())
                .map(|Outlining { ids, color, width }| (&ids[..], *color, *width));
            (looked.unwrap_or(seen), wanted.or(layers))
        };
        #[cfg(not(feature = "dev"))]
        let outline = layers;
        let (seer, viewpoint) = seen;
        let viewpoint = camera::checked(viewpoint, seer, &mut self.projection);
        frame.viewpoint = viewpoint;
        frame.origin = self.origin;
        self.origin = followed(self.origin, viewpoint.position);
        let perspective = matches!(viewpoint.projection, crate::Projection::Perspective { .. });
        frame.cut = self.seer != Some((seer, perspective));
        self.seer = Some((seer, perspective));
        (self.audio).hear_from(&frame.viewpoint, viewport.size, &self.world);
        frame.outline = outline.map(|(roots, color, width)| {
            let (walk, ids) = (&mut self.outline_walk, &mut frame.outline_ids);
            self.world.fill_subtree_ids(roots, walk, ids);
            (color, width)
        });
        #[cfg(feature = "dev")]
        {
            self.look.seen = Some(frame.viewpoint);
            frame.look = self.look.revision;
            frame.bare = self.look.bare;
            frame.show = self.look.shown();
            self.look.leave_out(frame);
            // As the look's viewpoint shows, so that a frame that turns to or from it is cut to.
            frame.looked = matches!(seer, Seer::Look(_));
        }
        frame.pixels = self.shown;
        if mem::take(&mut self.shown_new) {
            mem::swap(&mut frame.pixel_bytes, &mut self.shown_bytes);
            frame.pixels_new = true;
        }
        self.draws.append(&mut self.over);
        mem::swap(&mut frame.draws, &mut self.draws);
        self.draws.clear();
        self.canvas.place(viewport.size, &mut frame.canvas);
        self.uis.handed_off();
        self.loader.take(&mut self.textures);
        frame.loading = self.loader.pending();
        self.fonts.place(&mut self.textures);
        (frame.uploads).take(
            &mut self.meshes,
            &mut self.shaders,
            &mut self.textures,
            &mut self.materials,
        );
        frame.uploads.take_scene(&mut self.scene);
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
}

/// How far apart on each axis the points a frame's [origin](Context::origin) can be at are, and how
/// far on one the view goes from it before it moves: a power of two, so that the origin is exact
/// in `f32`, and positions within it of the origin step by less than 0.25 mm in `f32`.
const ORIGIN_STEP: f64 = 1024.0;

/// The origin a frame after one drawn from `origin` draws from, the view having seen from `at`:
/// `origin` while `at` is within [`ORIGIN_STEP`] of it on every axis, or isn't finite; else `at`
/// rounded to a multiple of it on each axis. So it moves at most once for each half a step the view
/// travels, and never for a view within a step of the world's origin.
fn followed(origin: DVec3, at: DVec3) -> DVec3 {
    let near = (at - origin).abs().cmple(DVec3::splat(ORIGIN_STEP)).all();
    if near || !at.is_finite() {
        return origin;
    }
    (at / ORIGIN_STEP).round() * ORIGIN_STEP
}

/// `transform`, which places something in the world, placing it relative to `origin` instead.
fn relative(transform: Mat4, origin: DVec3) -> Mat4 {
    let mut relative = transform;
    relative.w_axis -= origin.as_vec3().extend(0.0);
    relative
}

/// The refusal of the scene `scene` in a run without an asset root, as [`App::root`] gives one
/// (`scene.no_root`).
fn no_root(scene: &str) -> Error {
    let message = format!(
        "there's no asset root to find the scene {scene} under: `App::root` or `App::project` \
         gives the game one"
    );
    Error::new("scene.no_root", message)
}

/// The color of a cell in's edges, as [`Context::draw_cells`] draws them: a light blue, apart from
/// the grid's axes' colors.
#[cfg(feature = "dev")]
const CELL_IN: Vec4 = Vec4::new(0.3, 0.7, 1.0, 0.6);

/// The color of a cell refused's edges, as [`Context::draw_cells`] draws them.
#[cfg(feature = "dev")]
const CELL_REFUSED: Vec4 = Vec4::new(1.0, 0.25, 0.2, 0.9);

/// The radius of the look's viewpoint as a viewer while the run is stopped, at least, as
/// [`Context::stream_stopped`] says: a [`Viewer`](cells::Viewer)'s default.
#[cfg(feature = "dev")]
const VIEWED_RADIUS: f32 = 512.0;

/// Those of `roots`, sorted by cell, that are in `cell`.
fn roots_of(roots: &[(Cell, Entity)], cell: Cell) -> Vec<Entity> {
    let from = roots.partition_point(|&(other, _)| other < cell);
    let to = roots.partition_point(|&(other, _)| other <= cell);
    roots[from..to].iter().map(|&(_, root)| root).collect()
}

/// Logs `asset.texture_loaded` at the debug level, as a game may add many: texture `named`, `size`
/// pixels across and down, added from its file in the time since `started`. Its pixels decode on
/// the loader's threads from then on, as [`Loader`] logs; where the loader decodes inline, as a
/// test's or a replay's context's does, they decoded within that time.
fn texture_loaded(named: &str, size: [u32; 2], started: Instant) {
    debug!(
        code = "asset.texture_loaded",
        texture = named,
        ?size,
        ms = perf::millis(started.elapsed()),
        "Texture loaded"
    );
}

/// Adds `image`, sprite sheet `name`'s from `path`, read as far as its header, to `textures`
/// through `loader`, which decodes it, as a sheet of cells `cell` pixels across and down, if they
/// fit it, as [`fits`] checks; one that doesn't takes no texture.
fn add_sheet(
    loader: &mut Loader,
    textures: &mut Textures,
    name: &str,
    path: &Path,
    image: Image,
    cell: [u32; 2],
) -> Result<SpriteSheet> {
    let size = image.size;
    fits(name, path, size, cell)?;
    let texture = loader.load(textures, name, path, image, Mipmaps::None)?;
    Ok(SpriteSheet {
        texture,
        size,
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

/// The types a model's draws hold the bytes of, as its `asset` line says them after its path.
pub(crate) fn model_types() -> String {
    format!("{} {}", layout::<ModelVertex>(), layout::<ModelParams>())
}

/// Applies the patch `table` names, the simulation having run `ticks` ticks, as
/// [`patch::apply`](crate::patch::apply) does, and has `recorder`, if the run is recorded, write it
/// once it has applied, so that the recording's replay, which can't apply it, stops where the run's
/// code changed rather than going another way from there unsaid.
#[cfg(feature = "dev")]
fn patched(recorder: &mut Option<Recorder>, table: subsecond::JumpTable, ticks: u64) -> Result<()> {
    let library = table.lib.clone();
    crate::patch::apply(table, ticks)?;
    if let Some(recorder) = recorder {
        recorder.patched(&library);
    }
    Ok(())
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
    #[cfg(feature = "dev")]
    types: Vec<&'static str>,
    /// Where the overlays start.
    overlays: usize,
    /// What records the run, if it's recorded.
    recorder: Option<Recorder>,
    /// The session the run takes part in, if it's networked: a lockstep session's peer, a server,
    /// or a server's client.
    net: Option<Session>,
    /// What runs each tick again, if the run's ticks are sync-tested, until one goes another way.
    sync_test: Option<SyncTest>,
    /// Whether a layer ended the run with an error, as [`Context::fail`] says, which the context
    /// tells as the layers detach.
    failed: bool,
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
        #[cfg(feature = "dev")]
        self.types.insert(self.overlays, type_name::<L>());
        self.overlays += 1;
    }

    /// Adds an overlay on top of everything.
    pub(crate) fn push_overlay<L: Layer + 'static>(&mut self, overlay: L) {
        #[cfg(feature = "dev")]
        let overlay = Hot(overlay);
        self.layers.push(Box::new(overlay));
        self.names.push(short_name::<L>());
        #[cfg(feature = "dev")]
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

    /// Whether a recorder records the run.
    #[cfg(feature = "dev")]
    pub(crate) fn records(&self) -> bool {
        self.recorder.is_some()
    }

    /// The lockstep session the run is a peer of: for tests.
    #[cfg(test)]
    pub(crate) fn net_mut(&mut self) -> Option<&mut crate::net::Lockstep> {
        match &mut self.net {
            Some(Session::Peer(net)) => Some(net.as_mut()),
            _ => None,
        }
    }

    /// The session the run takes part in, if it's networked: for tests.
    #[cfg(test)]
    pub(crate) fn session_mut(&mut self) -> Option<&mut Session> {
        self.net.as_mut()
    }

    /// The queue of the layer at `index`, from the bottom, if it queues commands: for tests.
    #[cfg(test)]
    pub(crate) fn commands_of(&mut self, index: usize) -> Option<&mut dyn Commands> {
        self.layers.get_mut(index)?.commands()
    }

    /// What runs each tick again, if the run's ticks are sync-tested: for tests.
    #[cfg(test)]
    pub(crate) fn sync_test_mut(&mut self) -> Option<&mut SyncTest> {
        self.sync_test.as_mut()
    }

    /// Has the run be a peer of `net`'s lockstep session: for tests.
    #[cfg(test)]
    pub(crate) fn lockstep(&mut self, net: crate::net::Lockstep) {
        self.session(Session::Peer(Box::new(net)));
    }

    /// Has the run take part in `session`: as a lockstep session's peer, a server or a client.
    pub(crate) fn session(&mut self, session: Session) {
        self.net = Some(session);
    }

    /// Takes up what a layer asked of the run's session since the last frame, as
    /// [`Context::host`], [`Context::join`] and [`Context::leave`] say: between ticks, as a frame
    /// starts, before the session's turn. A session left logs what it carried and its delay, as one
    /// does as the run ends.
    pub(crate) fn take_asked(&mut self, cx: &mut Context) {
        let Some(asked) = cx.asked.take() else {
            return;
        };
        match asked {
            Asked::Start(session) => {
                if let Some(net) = self.net.take() {
                    net.ended();
                }
                let player = session.player();
                info!(player, "Started the session the game asked for");
                self.net = Some(*session);
            }
            Asked::Leave => {
                if let Some(net) = self.net.take() {
                    net.ended();
                    info!("Left the session, as the game asked");
                }
            }
        }
        self.tell_session(cx);
    }

    /// The run's session as a game's own UI shows it, as [`Session::status`] says: as a frame's
    /// turn leaves it.
    pub(crate) fn session_status(&self) -> Option<SessionStatus> {
        self.net.as_ref().and_then(Session::status)
    }

    /// Has each tick run again once it has run, a replay's too, as a SyncTest (`--sync-test`) runs
    /// it, as [`tick`](Self::tick) says: before the layers attach.
    pub(crate) fn sync_test(&mut self) {
        self.sync_test = Some(SyncTest::default());
    }

    /// Whether the run waits on its session: a joiner without the host's world, a tick without the
    /// other peer's inputs, or a client without the server's world or too far past its latest
    /// state, `next` being the next to run.
    pub(crate) fn net_waits(&self, next: u64) -> bool {
        self.net.as_ref().is_some_and(|net| !net.ready(next))
    }

    /// Waits out a frame until `until`: a server sending its clients' states as soon as its states
    /// thread has written them, as [`Server::wait`](crate::net::Server::wait) says; any other run
    /// asleep.
    pub(crate) fn net_wait(&mut self, until: Instant) {
        match &mut self.net {
            Some(Session::Server(server)) => server.wait(until),
            _ => std::thread::sleep(until.saturating_duration_since(Instant::now())),
        }
    }

    /// The time a frame `dt` after the last owes the simulation: a networked run's as
    /// [`Session::pace`] says, and any other run's `dt`.
    pub(crate) fn paced(&mut self, dt: Duration) -> Duration {
        self.net.as_mut().map_or(dt, |net| net.pace(dt))
    }

    /// Has a client's frame, `dt` after the last, draw the other players' entities as the server's
    /// states have them, as [`Client::interpolate`](crate::net::Client::interpolate) says, in place
    /// of where its ticks left them: before the world is drawn.
    pub(crate) fn interpolate(&mut self, cx: &mut Context, dt: Duration) {
        if let Some(Session::Client(client)) = &mut self.net {
            cx.past.instead(&cx.world, client.interpolate(dt));
        }
    }

    /// Between ticks, as a frame starts, the next tick to run being `next`: takes what the session
    /// sent since the last turn, as [`Session::turn`] says, sending the joiner or a client the
    /// world, if the host or the server owes it, or restoring the host's or the server's in place
    /// of the run's, the run's textures taking the names the host's or the server's gave them, as
    /// they do the names the host gave those it added since; and, in a rollback session or a
    /// client, takes the ticks that may run again, as [`Session::settle`] says, rolling back to the
    /// first to, as [`roll_back`](Self::roll_back) says, or a client's world taken to the server's
    /// where that's at or past `next`. Returns the tick the run's world is at once it restores
    /// another's or is taken past `next`, which its ticks go on from.
    pub(crate) fn net_turn(&mut self, cx: &mut Context, next: u64) -> Option<u64> {
        let mut net = self.net.take()?;
        let mut restored = None;
        match net.turn(next) {
            Turn::None => {}
            Turn::Send => {
                // A lockstep host's logs go, and the slots they kept vacant for their undos with
                // them, freed as the joiner's load of the world frees them, so that the peers give
                // the same slots to the entities spawned after; and no undo reaches past the world
                // the joiner starts from, which its own logs don't hold. Recorded, as an edit
                // between ticks, so that a replay drops them there too. A server's clients take
                // its world again with each state.
                if matches!(net, Session::Peer(_)) {
                    if let Some(recorder) = &mut self.recorder {
                        recorder.edited(&Edit::Renew.to_value(), &cx.world);
                    }
                    cx.apply_edit(Edit::Renew)
                        .expect("a renewal is never refused");
                }
                let scripts = cx.hosts.applied();
                let mods = cx.mods.applied();
                let (app, names) = (net.app(), &self.names);
                // The world whole for a joiner, and what each client sees for it.
                let mut write = |seen: Option<&crate::cells::Seen>| {
                    let of = crate::snapshot::Of {
                        world: &cx.world,
                        scenes: &cx.scenes.files,
                        cached: &cx.scenes.cached,
                        open: &cx.scenes.open,
                        scripts: &scripts,
                        mods: &mods,
                        seen,
                    };
                    let text = crate::snapshot::write(&of, &Run::Playing, app, names, next);
                    let unsent = |error: Error| {
                        error!(
                            code = error.code(),
                            "Can't send the joiner or a client the world: {error:#}"
                        );
                    };
                    text.map_err(unsent).ok()
                };
                net.send_world(&cx.world, &mut write, next);
            }
            Turn::Restore((tick, text)) => {
                let from = format!("the host's or the server's world at tick {tick}");
                let snapshot = Snapshot::parse(text, from, net.app());
                // A client's player is the server's choice, which its layers pick up as it
                // restores.
                cx.player = net.player();
                let ticks = snapshot.and_then(|snapshot| {
                    // Its textures go by the host's names, whatever ID files this run's disk holds.
                    #[cfg(feature = "dev")]
                    cx.name_as(
                        (snapshot.names.iter()).map(|(path, name)| (name.clone(), path.clone())),
                    );
                    cx.load_restored(snapshot.added());
                    // Its host's scripts, which its world's components are as, in place of its
                    // own files', and its host's mods in place of its own.
                    cx.hosts.take(&mut cx.world, &snapshot.scripts);
                    cx.mods.take(&mut cx.world, &snapshot.mods);
                    self.restore_in_place(cx, &snapshot)
                });
                match ticks {
                    Ok(ticks) => restored = Some(net.restored(ticks, &cx.world)),
                    Err(error) => {
                        error!(code = error.code(), "{error:#}");
                        net.lose("its world can't be restored");
                    }
                }
            }
        }
        #[cfg(feature = "dev")]
        if let Session::Peer(peer) = &mut net {
            let names = peer.take_names();
            if !names.is_empty() {
                cx.name_also(names);
            }
        }
        // The ticks that run again: a rollback peer's run on the other's inputs predicted whose
        // inputs have come, or a client's past the server's latest state.
        let next = restored.unwrap_or(next);
        let from = net.settle(next, &cx.world);
        self.net = Some(net);
        match from {
            Some(from) if from < next => self.roll_back(cx, from, next),
            // A client's world, taken to the server's state after the tick before `next`, or past.
            Some(from) => {
                let net = self.net.as_mut().expect("it's back");
                cx.world.rewind(net.start_of(from));
                cx.share_assets();
                net.rolled_back(from, from);
                if from > next {
                    restored = Some(from);
                }
            }
            None => {}
        }
        // A rollback peer's ticks run on the other's inputs predicted that held nothing are final
        // once theirs have come, though they don't run again: the world as the first not yet so
        // started is.
        if let Some(Session::Peer(peer)) = &self.net {
            cx.confirm_scene_from(peer.final_world());
        }
        restored
    }

    /// Tells `cx` the session the run takes part in, if any, as [`Context::player`] and
    /// [`Context::networked`] say, and whether it's a server's or a client's: as the layers attach
    /// or pick up from a restore.
    fn tell_session(&self, cx: &mut Context) {
        cx.player = self.net.as_ref().and_then(Session::player);
        cx.networked = self.net.is_some();
        cx.served = self.served();
        cx.client = matches!(self.net, Some(Session::Client(_)));
        cx.follows |= self.joins();
    }

    /// Whether the run hosts or plays alone, rather than joining a lockstep session or connecting
    /// to a server, whose host or server alone watches the scene and script files the layers
    /// loaded.
    #[cfg(feature = "dev")]
    pub(crate) fn watches_files(&self) -> bool {
        !self.joins()
    }

    /// Whether the run joins a lockstep session or connects to a server, going as the host's or the
    /// server's world goes.
    fn joins(&self) -> bool {
        self.net.as_ref().is_some_and(Session::follows)
    }

    /// Whether the run serves a server's session (`--serve`) or connects to one (`--connect`).
    fn served(&self) -> bool {
        matches!(self.net, Some(Session::Server(_) | Session::Client(_)))
    }

    /// Whether the run is networked, hosting, joining, serving or connecting, whose scene files the
    /// session's others' must stay as.
    #[cfg(feature = "dev")]
    pub(crate) fn is_peer(&self) -> bool {
        self.net.is_some()
    }

    /// Applies `edit`, an author's from outside the layers, between ticks, as [`edit`](Self::edit)
    /// does; or, in a lockstep session, offers it for the tick both peers apply it before, giving
    /// its ticket. A server's client refuses it (`net.no_edit`), as the server's next state would
    /// take the edit away.
    #[cfg(feature = "dev")]
    pub(crate) fn offer(&mut self, cx: &mut Context, edit: Edit) -> Offered {
        match &mut self.net {
            Some(Session::Peer(net)) => Offered::Later(net.offer(edit)),
            Some(Session::Client(_)) => Offered::Now(Err(Error::new(
                "net.no_edit",
                "a client's world goes as its server's, whose next state would take an edit of \
                 it away: edit the server's world",
            ))),
            Some(Session::Server(_)) | None => {
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
            Some(Session::Peer(net)) => {
                // What it names that isn't loaded is loaded now, so that the names the host gives
                // those go out before it. One naming a texture that can't be, which the joiner
                // might stand in for, isn't offered, warning as a save that doesn't load does.
                if let Ok(missing) = cx.load_scene_assets(&text)
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
            _ => {
                let edit = &mut |cx: &mut Context, edit| self.edit(cx, edit);
                let _ = crate::scene_files::apply_save(cx, edit, &author, text);
            }
        }
    }

    /// Reloads the script whose path from the project's root is `path`, its text now `text`, as
    /// its file is saved, through an edit, between ticks, as [`edit`](Self::edit) applies it; or,
    /// in a lockstep session, offers that edit for the tick both peers apply it before. Either
    /// way, what it did is logged, as [`Hosts::reload`](crate::luau::Hosts::reload) says.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_script(&mut self, cx: &mut Context, path: String, text: String) {
        let edit = Edit::Script { path, text };
        match &mut self.net {
            Some(Session::Peer(net)) => net.offer_unanswered(edit),
            _ => {
                let _ = self.edit(cx, edit);
            }
        }
    }

    /// Offers the patch `table` names, as
    /// [`Lockstep::offer_patch`](crate::net::Lockstep::offer_patch) does; `None` outside a
    /// lockstep session, where it applies now.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_patch(
        &mut self,
        table: subsecond::JumpTable,
    ) -> std::result::Result<Ticket, subsecond::JumpTable> {
        match &mut self.net {
            Some(Session::Peer(net)) => Ok(net.offer_patch(table)),
            _ => Err(table),
        }
    }

    /// Applies the patch `table` names, the simulation having run `ticks` ticks, as
    /// [`patch::apply`](crate::patch::apply) does, outside a lockstep session, which
    /// [`offer_patch`](Self::offer_patch) offers it to; a recording gets it once it has applied,
    /// as [`Recorder::patched`] says.
    #[cfg(feature = "dev")]
    pub(crate) fn patch(&mut self, table: subsecond::JumpTable, ticks: u64) -> Result<()> {
        patched(&mut self.recorder, table, ticks)
    }

    /// What the edit or the patch offered with `ticket` did, and the tick it applied before, once
    /// it has, which it gives once.
    #[cfg(feature = "dev")]
    pub(crate) fn offered(&mut self, ticket: Ticket) -> Option<(u64, Result<Done>)> {
        match &mut self.net {
            Some(Session::Peer(net)) => net.done(ticket),
            _ => None,
        }
    }

    /// Whether the edit or the patch offered with `ticket` has applied, or been refused.
    #[cfg(feature = "dev")]
    pub(crate) fn offered_is_done(&self, ticket: Ticket) -> bool {
        matches!(&self.net, Some(Session::Peer(net)) if net.is_done(ticket))
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
        self.tell_session(cx);
        #[cfg(feature = "dev")]
        {
            cx.sync_tested = self.sync_test.is_some();
        }
        self.load(cx);
        #[cfg(feature = "dev")]
        cx.read_scenes();
        for layer in &mut self.layers {
            layer.attach(cx);
            cx.share_assets();
        }
        cx.attached = true;
        if let Some(recorder) = &mut self.recorder {
            recorder.attached(&cx.world, &cx.mods.applied());
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
        self.tell_session(cx);
        #[cfg(feature = "dev")]
        {
            cx.sync_tested = self.sync_test.is_some();
        }
        self.load(cx);
        cx.attached = true;
        cx.load_restored(snapshot.added());
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
                    cached: cx.scenes.cached.clone(),
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
            Some("the run is networked, and its world would part from the session's others")
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
            cached: cx.scenes.cached.clone(),
            open: cx.scenes.open.clone(),
            authors: mem::take(&mut cx.authors),
        });
        info!(ticks, entities, "Playing a copy of the world as edited");
    }

    /// Brings back the world as edited that [`play`](Self::play) holds aside, if it holds one, as
    /// the tool API's `stop` asks: the very world, with its scene files, its open scene and the
    /// authors' edits of it, the copy and all done to it going, but for the scripts reloaded since,
    /// whose components it holds as they declare them now, as [`Scripts`](crate::Scripts) says;
    /// empties each layer's queue, which
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
        cx.scenes.cached = edited.cached;
        cx.scenes.open = edited.open;
        // Of the world as edited, as its scene files and open scene are.
        cx.seen_scene();
        // Its scripts' components as their scripts declare them now, though they reloaded since.
        cx.hosts.migrate(&mut cx.world);
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
        cx.hosts.migrate(&mut cx.world);
        for layer in &mut self.layers {
            layer.restored(cx);
        }
        Ok(())
    }

    /// Puts `snapshot`'s world in `cx`'s place, the layers having loaded, with its scene files and
    /// its open scene, or the start scene's file open for a snapshot that holds none, its scripts'
    /// components as the layers' scripts declare them, and has each layer pick up from it, from the
    /// bottom up; returns the ticks the simulation had run as it was taken.
    ///
    /// Fails as [`Snapshot::restore`] does.
    fn restore_in_place(&mut self, cx: &mut Context, snapshot: &Snapshot) -> Result<u64> {
        let ticks = snapshot.restore(&mut cx.world, &self.names)?;
        cx.scenes.files.clone_from(&snapshot.scenes);
        cx.scenes.cached.clone_from(&snapshot.cached);
        cx.seen_scene();
        cx.scenes.open = (snapshot.open.clone())
            .unwrap_or_else(|| scene_files::opened(cx.start.as_deref(), &snapshot.scenes));
        #[cfg(feature = "dev")]
        cx.scenes.changed();
        cx.hosts.migrate(&mut cx.world);
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

    /// Detaches from the top down, the reverse of attaching, once a recording has its end, a
    /// SyncTest that found no tick going another way has logged how many it ran again
    /// (`sync_test.matched`), and a rollback session's peer how it rolled back (`net.rollback`);
    /// and takes from `cx` whether a layer ended the run with an error, as
    /// [`failed`](Self::failed) tells, as it detached too.
    pub(crate) fn detach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        if let Some(recorder) = &mut self.recorder {
            recorder.end();
        }
        if let Some(sync_test) = &self.sync_test {
            info!(
                code = "sync_test.matched",
                ticks = sync_test.matched(),
                "Each tick ran again on a copy of the world taken as it started, and left it as it \
                 left the world"
            );
        }
        if let Some(net) = &self.net {
            net.ended();
        }
        cx.attached = false;
        cx.authors.clear(&mut cx.world);
        for layer in self.layers.iter_mut().rev() {
            layer.detach(cx);
        }
        self.failed |= cx.failed;
    }

    /// Whether a layer ended the run with an error, as [`Context::fail`] says, once the layers have
    /// detached, which makes the run's status 1.
    pub(crate) fn failed(&self) -> bool {
        self.failed
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

    /// Runs a tick of the simulation, as [`run`](Self::run) says. A tick waits for the cells it
    /// brings in as it starts to be prepared, returning `false` without running until they are, as
    /// [`Context::cells_ready`] says. In a lockstep session, the tick also waits for the other
    /// peer's inputs, or, in a rollback session, until it's near enough them, and a client's for the
    /// server's world, and while it's too far past the latest state, as [`Session::ready`] says;
    /// once it can run, the run's queued commands go out as its inputs, as [`Session::start_tick`]
    /// says.
    pub(crate) fn tick(&mut self, cx: &mut Context, tick: Tick) -> bool {
        let _zone = gg_core::zone!();
        if !cx.cells_ready() {
            return false;
        }
        let Self {
            layers,
            names,
            recorder,
            net,
            sync_test,
            ..
        } = self;
        if let Some(net) = net.as_mut() {
            if !net.ready(tick.number) {
                net.waited(tick.number);
                return false;
            }
            let queues = layers.iter_mut().enumerate();
            let queues = queues.filter_map(|(index, layer)| Some((index, layer.commands()?)));
            net.start_tick(tick.number, queues, &cx.world);
        }
        Self::run(layers, names, recorder, net.as_mut(), sync_test, cx, tick);
        true
    }

    /// Runs tick `tick`: each layer's, from the bottom up, after which the world shares the
    /// context's assets again, as [`Context::share_assets`] says; then the world's cells come in and
    /// go out, as [`Context::stream_cells`] says, as they do wherever a tick runs; and then the
    /// sounds the tick queued play, as [`World::play_sound`] says. A recording gets each layer's
    /// commands just before its tick applies them, so that a tick that panics leaves them, and the
    /// world's hash after the last. In a networked session, the tick runs on the session's inputs
    /// for it, as [`Session::inputs`] says: in a lockstep session, both peers', the edits among
    /// them applying first, as a recording keeps them; or, in a rollback session, before the
    /// other's have come, on them predicted, a copy of the world kept as it starts, which a
    /// rollback runs it again from; a server's own and its clients'; and a client's own. Where the
    /// tick may run again, as a rollback peer's or a client's, the sounds it plays are kept, which
    /// a run again doesn't play again, as [`Context::play_queued_again`] says; and the session is
    /// told once it has run, as [`Session::ran`] says. A SyncTest takes a copy of the world once
    /// the edits have applied, keeps each layer's commands as its tick is about to apply them, and
    /// runs the tick again once it has run, as [`run_again`](Self::run_again) says: one that fails
    /// ends the run with its error, as [`Context::fail`] does, and the SyncTest with it.
    fn run(
        layers: &mut [Box<dyn Layer>],
        names: &[&'static str],
        recorder: &mut Option<Recorder>,
        mut net: Option<&mut Session>,
        sync_test: &mut Option<SyncTest>,
        cx: &mut Context,
        tick: Tick,
    ) {
        // A client's ticks may all run again, as may a rollback peer's on the other's inputs
        // predicted, whose world before them is then final no more than after.
        let final_tick = match net.as_deref() {
            None | Some(Session::Server(_)) => true,
            Some(Session::Peer(peer)) => peer.settled(tick.number),
            Some(Session::Client(_)) => false,
        };
        cx.drop_untimely_scene();
        if final_tick {
            cx.confirm_scene();
        }
        let merged = match net.as_deref_mut() {
            Some(net) => {
                let merged = net.inputs(tick.number, cx);
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
                    let patched = patched(recorder, table, tick.number);
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
        if let Some(sync_test) = sync_test.as_mut() {
            sync_test.take(&cx.world);
        }
        cx.enter_coming();
        if let Some(recorder) = recorder.as_mut() {
            for &(cell, digest) in &cx.cells.came {
                recorder.cell_came(cell, digest);
            }
        }
        for (index, layer) in layers.iter_mut().enumerate() {
            if let Some(merged) = &merged
                && let Some(queue) = layer.commands()
            {
                merged.fill(index, names[index], queue, cx);
            }
            if let Some(recorder) = recorder.as_mut()
                && let Some(commands) = layer.commands()
            {
                recorder.queued(index, names[index], commands, &cx.world);
            }
            if let Some(sync_test) = sync_test.as_mut()
                && let Some(commands) = layer.commands()
            {
                sync_test.queued(index, names[index], commands, &cx.world);
            }
            layer.tick(&mut cx.world, tick);
            cx.change_asked_scene();
            cx.share_assets();
        }
        cx.stream_cells();
        match net.as_deref_mut().and_then(|net| net.played(tick.number)) {
            Some(played) => cx.play_queued_again(played),
            None => cx.play_queued(),
        }
        if let Some(again) = sync_test.as_mut()
            && let Err(error) = Self::run_again(layers, again, cx, tick)
        {
            error!(code = error.code(), tick = tick.number, "{error:#}");
            // As `Context::fail` ends it, with the error logged.
            cx.exiting = true;
            cx.failed = true;
            *sync_test = None;
        }
        if final_tick {
            cx.confirm_scene();
        }
        // A rollback session's isn't recorded, as `--rollback` takes no `--record`, nor a
        // client's, as `--connect` takes none.
        if let Some(recorder) = recorder {
            recorder.ticked(tick, &cx.world);
        }
        if let Some(net) = net {
            net.ran(tick.number, &mut cx.world);
        }
    }

    /// Takes the world back to where tick `from` started, as [`Session::start_of`] has it, the
    /// start of a tick run on the other lockstep peer's inputs predicted, which held something, or
    /// a client's server's world after the tick before, and runs it and the ticks after again, to
    /// `next`, the next to run, as rollback does, the world going on as they leave it, as
    /// [`run`](Self::run) runs each: the very world, as [`World::rewind`] says, so that what's kept
    /// of it holds, and the layers are told nothing. Each layer's queue, which holds what it queued
    /// since the last tick, is as it was once they've run; and the frames after draw on from where
    /// the last started.
    fn roll_back(&mut self, cx: &mut Context, from: u64, next: u64) {
        let _zone = gg_core::zone!();
        let Self {
            layers,
            names,
            recorder,
            net,
            sync_test,
            ..
        } = self;
        let Some(net) = net.as_mut() else { return };
        cx.world.rewind(net.start_of(from));
        let queued: Vec<(usize, Vec<Value>)> = (layers.iter_mut().enumerate())
            .filter_map(|(index, layer)| {
                let queue = layer.commands()?;
                Some((index, (0..queue.len()).map(|i| queue.value(i)).collect()))
            })
            .collect();
        for number in from..next {
            if number + 1 == next {
                cx.capture();
            }
            let tick = Tick { number };
            Self::run(
                layers,
                names,
                recorder,
                Some(&mut *net),
                sync_test,
                cx,
                tick,
            );
        }
        for (index, values) in queued {
            let queue = layers[index].commands().expect("it queued them");
            queue.clear();
            for value in &values {
                let pushed = queue.push(value);
                pushed.expect("a queue queues again what it held");
            }
        }
        net.rolled_back(from, next);
    }

    /// Runs a tick as a replay does: as [`tick`](Self::tick) does, but never waiting, the cells it
    /// brings in prepared then and there unless the tick before prepared them, with each layer's
    /// queue emptied and filled again by `fill`, which gets the layer's index, its queue, if it has
    /// one, and the world as its tick finds it; and a SyncTest runs it again.
    ///
    /// Fails as `fill` does, and as [`run_again`](Self::run_again) does, which ends the SyncTest.
    pub(crate) fn replay_tick(
        &mut self,
        cx: &mut Context,
        tick: Tick,
        mut fill: impl FnMut(usize, Option<&mut dyn Commands>, &World) -> Result<()>,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
        let Self {
            layers,
            names,
            sync_test,
            ..
        } = self;
        cx.drop_untimely_scene();
        cx.confirm_scene();
        // What the tick before prepared ahead, on the replay's own thread.
        cx.prepare_cells();
        if let Some(sync_test) = sync_test.as_mut() {
            sync_test.take(&cx.world);
        }
        cx.enter_coming();
        for (index, layer) in layers.iter_mut().enumerate() {
            fill(index, layer.commands(), &cx.world)?;
            if let Some(sync_test) = sync_test.as_mut()
                && let Some(commands) = layer.commands()
            {
                sync_test.queued(index, names[index], commands, &cx.world);
            }
            layer.tick(&mut cx.world, tick);
            cx.change_asked_scene();
            cx.share_assets();
        }
        cx.stream_cells();
        cx.play_queued();
        if let Some(again) = sync_test.as_mut()
            && let Err(error) = Self::run_again(layers, again, cx, tick)
        {
            *sync_test = None;
            return Err(error);
        }
        cx.confirm_scene();
        Ok(())
    }

    /// Runs `tick` again, as a SyncTest does once it has run, `sync_test` having kept a copy of the
    /// world as it started and each layer's commands as its tick applied them: on the copy, put in
    /// the world's place, each layer told as it's put there and as the world comes back, as
    /// [`Layer::swapped`] says, and given back the commands its tick applied; the sounds the tick
    /// queues there stay in the copy, which nothing plays, as they played as it ran on the world.
    /// Then checks that it left the copy as it left the world.
    ///
    /// Fails, without running it again, as [`SyncTest::refused`] does, and as
    /// [`SyncTest::compare`] does.
    fn run_again(
        layers: &mut [Box<dyn Layer>],
        sync_test: &mut SyncTest,
        cx: &mut Context,
        tick: Tick,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
        sync_test.refused()?;
        mem::swap(&mut cx.world, sync_test.world_mut());
        for layer in layers.iter_mut() {
            layer.swapped(cx);
        }
        cx.enter_coming();
        for (index, layer) in layers.iter_mut().enumerate() {
            if let Some(queue) = layer.commands() {
                sync_test.give_back(index, queue);
            }
            layer.tick(&mut cx.world, tick);
            cx.change_asked_scene();
            cx.share_assets();
        }
        cx.stream_cells();
        mem::swap(&mut cx.world, sync_test.world_mut());
        for layer in layers.iter_mut() {
            layer.swapped(cx);
        }
        sync_test.compare(tick, &cx.world)
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
    #[cfg(feature = "dev")]
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
    /// its press changes nothing. A game UI that takes the mouse's buttons or its wheel from the
    /// layers below, as [`GameUi::event`] says, handles them for the layer that sent it them.
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
            cx.uis.forget_consumed();
            // A UI a layer sent the event to took it, if the layer let it go on.
            if layer.event(cx, event).is_break() || cx.uis.consumed() {
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

    fn swapped(&mut self, cx: &mut Context) {
        HotFn::current(L::swapped).call((&mut self.0, cx));
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
    use crate::{Fog, Key, Projection, Transform, render_thread};

    /// A frame of `cx`'s world, drawn and handed off with its view at 64 by 32.
    fn handed_off(cx: &mut Context) -> Extracted {
        cx.draw_world(1.0, 0.0, 0);
        let mut frame = Extracted::default();
        let view = Viewport {
            origin: [0, 0],
            size: [64, 32],
        };
        cx.extract(&mut frame, view);
        frame
    }

    /// A frame of `cx`'s world, as [`handed_off`] has it: whether it's cut to its viewpoint.
    fn cut(cx: &mut Context) -> bool {
        handed_off(cx).cut
    }

    /// Where the look's grid draws, each cell in draws its cube's twelve edges, and each cell
    /// refused; without the grid, none.
    #[cfg(feature = "dev")]
    #[test]
    fn the_cells_draw_where_the_look_s_grid_does() {
        use gg_core::math::IVec3;

        let mut cx = Context::seeded(1);
        let world = cx.world_mut();
        world.spawn(crate::Cells {
            folder: "cells".to_owned(),
            size: 10.0,
        });
        world.insert_resource(CellState {
            cells_in: [Cell(IVec3::ZERO), Cell(IVec3::X)].into(),
            refused: [Cell(IVec3::Y)].into(),
            ..CellState::default()
        });
        let lines = |cx: &mut Context| {
            let before = cx.over.len();
            cx.draw_world(1.0, 0.0, 0);
            cx.over.len() - before
        };
        assert_eq!(lines(&mut cx), 0, "no grid");
        cx.look.grid = Some(Plane::XZ);
        assert_eq!(lines(&mut cx), 3 * 12);
    }

    /// The frame's origin stays at the world's while the view is within 1,024 of it on every axis,
    /// and from the frame after one whose view went past, is where the view was, rounded to a
    /// multiple of 1,024 on each axis, the frame that went past drawing from where it was; what a
    /// layer draws in the world is drawn from it; a view that isn't finite leaves it; and in a
    /// development build, the look's viewpoint moves it as a frame starts.
    #[test]
    fn the_origin_follows_the_view_a_step_of_1024_at_a_time() {
        let x = |x| DVec3::new(x, 0.0, 0.0);
        let near = DVec3::new(1024.0, -1024.0, 3.0);
        assert_eq!(followed(DVec3::ZERO, near), DVec3::ZERO);
        let past = DVec3::new(1024.5, 0.0, -1500.0);
        assert_eq!(
            followed(DVec3::ZERO, past),
            DVec3::new(1024.0, 0.0, -1024.0)
        );
        assert_eq!(followed(x(1024.0), x(2048.0)), x(1024.0));
        assert_eq!(followed(x(1024.0), x(2049.0)), x(2048.0));
        for broken in [DVec3::new(f64::NAN, 1.0e9, 0.0), x(f64::INFINITY)] {
            assert_eq!(followed(x(1024.0), broken), x(1024.0), "{broken}");
        }

        let mut cx = Context::seeded(1);
        let far = Viewpoint {
            position: DVec3::new(1.0e6 + 0.3, 0.0, 2.0),
            ..Viewpoint::default()
        };
        cx.set_viewpoint(far);
        assert_eq!(
            handed_off(&mut cx).origin,
            DVec3::ZERO,
            "the frame that went past"
        );
        let origin = x(1_000_448.0);
        assert_eq!(cx.origin(), origin);
        cx.set_viewpoint(far);
        let at = Mat4::from_translation(Vec3::new(1_000_448.25, 0.0, 0.0));
        cx.draw_sprite(at, Sprite::default());
        let frame = handed_off(&mut cx);
        assert_eq!(frame.origin, origin);
        let mut expected = Draws::default();
        let relative = Mat4::from_translation(Vec3::new(0.25, 0.0, 0.0));
        expected.push_sprite(relative, Sprite::default());
        expected.draw_scene();
        assert_eq!(frame.draws, expected);
        #[cfg(feature = "dev")]
        {
            let looked = DVec3::new(-5000.0, 0.0, 0.0);
            cx.look.viewpoint = Some(Viewpoint {
                position: looked,
                ..far
            });
            cx.follow_look();
            assert_eq!(cx.origin(), x(-5120.0));
            assert_eq!(handed_off(&mut cx).origin, x(-5120.0));
        }
    }

    /// The view is antialiased as a layer sets, in place of its world's environment's, and of none
    /// for a world without one, and as the environment says again once it's unset; the world, its
    /// environment and its hash among it, stays as it was.
    #[test]
    fn a_layers_antialiasing_takes_the_environments_place() {
        let antialiased = |cx: &mut Context| handed_off(cx).antialiasing;
        let mut cx = Context::seeded(1);
        assert_eq!(antialiased(&mut cx), Antialiasing::None, "no environment");
        cx.set_antialiasing(Some(Antialiasing::Smaa));
        assert_eq!(antialiased(&mut cx), Antialiasing::Smaa, "in place of none");
        let environment = cx.world_mut().spawn(Environment::default());
        let hash = cx.world().hash();
        assert_eq!(antialiased(&mut cx), Antialiasing::Smaa, "in place of TAA");
        cx.set_antialiasing(Some(Antialiasing::None));
        assert_eq!(antialiased(&mut cx), Antialiasing::None);
        assert_eq!(cx.antialiasing(), Some(Antialiasing::None));
        let held = cx.world().get::<Environment>(environment).copied();
        assert_eq!(held, Some(Environment::default()));
        assert_eq!(cx.world().hash(), hash);
        cx.set_antialiasing(None);
        assert_eq!(antialiased(&mut cx), Antialiasing::Taa, "the environment's");
    }

    /// A camera at `x` along X, of perspective, primary or not.
    fn camera_at(x: f32, primary: bool) -> (Transform, Camera) {
        let at = Transform {
            translation: DVec3::new(f64::from(x), 0.0, 0.0),
            ..Transform::IDENTITY
        };
        let projection = Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        };
        (
            at,
            Camera {
                projection,
                primary,
            },
        )
    }

    /// A projection set from code that can't project, as a near plane of 0 or a field of view that
    /// isn't a number, which would panic the render thread, is drawn through the last that could,
    /// or the default's, from where the camera is, and warns once (`engine.bad_projection`) until
    /// one can again.
    #[test]
    fn a_projection_that_cant_project_is_replaced_by_the_last_that_could_with_a_warning() {
        let logged = crate::testing::logged();
        let mut cx = Context::seeded(1);
        let good = Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        };
        let camera = cx.world_mut().spawn((
            Transform {
                translation: DVec3::X,
                ..Transform::IDENTITY
            },
            Camera {
                projection: Projection::Perspective {
                    fov: f32::NAN,
                    near: 0.1,
                },
                primary: true,
            },
        ));
        let seen = |cx: &mut Context| handed_off(cx).viewpoint;
        let warnings = || logged.codes.try_iter().collect::<Vec<_>>();
        let first = seen(&mut cx);
        assert_eq!(first.projection, Viewpoint::default().projection, "no last");
        assert_eq!(first.position, DVec3::X, "from where the camera is");
        assert_eq!(warnings(), ["engine.bad_projection"]);
        let set = |cx: &mut Context, projection| {
            let camera = cx.world_mut().get_mut::<Camera>(camera).expect("alive");
            camera.projection = projection;
        };
        set(&mut cx, good);
        assert_eq!(seen(&mut cx).projection, good);
        assert!(warnings().is_empty());
        let near_nothing = Projection::Perspective {
            fov: 1.0,
            near: 0.0,
        };
        set(&mut cx, near_nothing);
        assert_eq!(seen(&mut cx).projection, good, "the last that could");
        assert_eq!(seen(&mut cx).projection, good);
        assert_eq!(warnings(), ["engine.bad_projection"], "once");
        // A layer's viewpoint, which a game sets whole, as the frame takes it: of a source of its
        // own, which has no good projection of the camera's to fall back to, and warns anew, as
        // does the camera as the view comes back to it.
        let flat = Viewpoint::default().projection;
        cx.set_viewpoint(Viewpoint {
            projection: Projection::Orthographic {
                height: 0.0,
                near: -1.0,
                far: 1.0,
            },
            ..Viewpoint::default()
        });
        assert_eq!(seen(&mut cx).projection, flat, "not the camera's");
        assert_eq!(warnings(), ["engine.bad_projection"], "a layer's own");
        assert_eq!(seen(&mut cx).projection, flat, "the camera, with none good");
        assert_eq!(warnings(), ["engine.bad_projection"], "the camera's own");
        set(&mut cx, good);
        assert_eq!(seen(&mut cx).projection, good);
        assert!(warnings().is_empty());
    }

    /// Fog with a number that isn't finite, set from code, would turn the whole view black: the
    /// frame is drawn without it, with one warning (`engine.bad_fog`) for as long as it lasts.
    #[test]
    fn fog_that_isnt_finite_is_left_out_of_the_frame_with_a_warning() {
        let logged = crate::testing::logged();
        let mut cx = Context::seeded(1);
        let environment = |fog| Environment {
            fog: Some(fog),
            ..Environment::default()
        };
        let broken = Fog {
            falloff: f32::NAN,
            ..Fog::default()
        };
        let id = cx.world_mut().spawn(environment(broken));
        let fog = |cx: &mut Context| handed_off(cx).fog;
        let warnings = || logged.codes.try_iter().collect::<Vec<_>>();
        assert_eq!(fog(&mut cx), None);
        assert_eq!(fog(&mut cx), None);
        assert_eq!(warnings(), ["engine.bad_fog"], "once");
        *cx.world_mut().get_mut::<Environment>(id).expect("alive") = environment(Fog::default());
        assert_eq!(fog(&mut cx), Some(Fog::default()));
        assert!(warnings().is_empty());
        let infinite = Fog {
            density: f32::INFINITY,
            ..Fog::default()
        };
        *cx.world_mut().get_mut::<Environment>(id).expect("alive") = environment(infinite);
        assert_eq!(fog(&mut cx), None);
        assert_eq!(warnings(), ["engine.bad_fog"], "anew after good fog");
    }

    /// A layer that sends each event to its UI and lets it go on, as a game that doesn't ask what
    /// the UI did.
    struct Sends(crate::GameUi);

    impl Layer for Sends {
        fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
            self.0.event(cx, event);
            ControlFlow::Continue(())
        }
    }

    /// A game UI takes the mouse's presses and wheel over its buttons and lists from the layers
    /// below the layer that sent it them, which see those elsewhere, and releases go on; whether
    /// the cursor is over it is asked of the UI and of the context.
    #[test]
    fn a_game_ui_takes_the_mouse_over_what_it_draws_from_the_layers_below() {
        use crate::MouseButton;

        let scratch = crate::dirs::tests::Scratch::new("ui-takes-the-mouse");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let path = scratch.0.join("menu.ui");
        // A button in the view's middle, a list in its top left corner and a text in its top right,
        // in canvas units of a view 1,920 by 1,080 pixels.
        let file = r#"(nodes: [
            Button(name: "play", width: Fixed(400.0), height: Fixed(80.0)),
            List(
                name: "scores",
                anchor: (Start, Start),
                width: Fixed(300.0),
                height: Fixed(200.0),
                children: [Text(name: "score", text: "120")],
            ),
            Text(name: "title", anchor: (End, Start), text: "TITLE"),
        ])"#;
        std::fs::write(&path, file).expect("written");
        let mut cx = Context::seeded(1);
        let ui = cx.add_ui("menu", &path).expect("added");
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut stack = Stack::default();
        stack.push_layer(Probe {
            name: "world",
            handles: false,
            calls: calls.clone(),
        });
        stack.push_layer(Sends(ui));
        // Drawn and handed off, so that the view is known and the UI drew in the last frame.
        ui.draw(&mut cx);
        let mut frame = Extracted::default();
        let view = Viewport {
            origin: [0, 0],
            size: [1920, 1080],
        };
        cx.extract(&mut frame, view);
        let button = |pressed| Event::MouseButton {
            button: MouseButton::Left,
            pressed,
        };
        let wheel = Event::Scroll(crate::Scroll::Lines { x: 0.0, y: -1.0 });
        let reached = || !calls.borrow().is_empty();
        let at = |cx: &mut Context, stack: &mut Stack, x: f64, y: f64| {
            let _ = stack.event(cx, &Event::CursorMoved { x, y });
            calls.borrow_mut().clear();
        };

        at(&mut cx, &mut stack, 960.0, 540.0);
        assert!(ui.pointer_over(&mut cx), "over the button");
        assert!(cx.pointer_over_ui());
        let handled = stack.event(&mut cx, &button(true));
        assert_eq!(handled, ControlFlow::Break(()), "a press over a button");
        assert!(!reached(), "a press over a button reaches the world");
        // A release goes on, as the layers below let go of what they saw pressed.
        let _ = stack.event(&mut cx, &button(false));
        assert!(reached(), "a release goes on down");
        calls.borrow_mut().clear();

        at(&mut cx, &mut stack, 100.0, 100.0);
        assert!(ui.pointer_over(&mut cx) && cx.pointer_over_ui(), "the list");
        let _ = stack.event(&mut cx, &wheel);
        assert!(!reached(), "the wheel over a list reaches the world");

        for (x, y, what) in [(960.0, 100.0, "nothing"), (1850.0, 20.0, "a text")] {
            at(&mut cx, &mut stack, x, y);
            assert!(!ui.pointer_over(&mut cx) && !cx.pointer_over_ui(), "{what}");
            let handled = stack.event(&mut cx, &button(true));
            assert_eq!(handled, ControlFlow::Continue(()), "{what}");
            assert!(reached(), "a press over {what} doesn't reach the world");
            calls.borrow_mut().clear();
            let _ = stack.event(&mut cx, &wheel);
            assert!(reached(), "the wheel over {what} doesn't reach the world");
            calls.borrow_mut().clear();
        }

        // Not drawn for two frames: it takes nothing and is over nothing.
        at(&mut cx, &mut stack, 960.0, 540.0);
        cx.extract(&mut frame, view);
        cx.extract(&mut frame, view);
        assert!(!ui.pointer_over(&mut cx) && !cx.pointer_over_ui());
        let _ = stack.event(&mut cx, &button(true));
        assert!(reached(), "a UI that no longer draws takes no press");
        calls.borrow_mut().clear();
        ui.draw(&mut cx);
        assert!(ui.pointer_over(&mut cx), "drawn again");

        // Input off: it takes nothing and is over nothing.
        ui.set_input(&mut cx, false);
        assert!(!ui.pointer_over(&mut cx) && !cx.pointer_over_ui());
        let _ = stack.event(&mut cx, &button(true));
        assert!(reached(), "a UI without input takes no press");
    }

    /// The view is cut to its viewpoint as it comes from elsewhere than the last frame's, which
    /// motion blur takes as standing still: in the first frame; as the primary camera switches, not
    /// as it moves; as its projection turns to the other kind; and as a layer sets a viewpoint and
    /// as it stops, not as the one it sets moves.
    #[test]
    fn the_view_cuts_as_its_viewpoint_comes_from_elsewhere() {
        let mut cx = Context::seeded(1);
        let first = cx.world_mut().spawn(camera_at(0.0, true));
        let second = cx.world_mut().spawn(camera_at(9.0, false));
        assert!(cut(&mut cx), "the first frame isn't cut to");
        assert!(!cut(&mut cx), "the same camera cuts");
        cx.world_mut()
            .get_mut::<Transform>(first)
            .expect("alive")
            .translation
            .x = 5.0;
        assert!(!cut(&mut cx), "the camera moving cuts");
        cx.world_mut()
            .get_mut::<Camera>(first)
            .expect("alive")
            .primary = false;
        cx.world_mut()
            .get_mut::<Camera>(second)
            .expect("alive")
            .primary = true;
        assert!(cut(&mut cx), "the primary camera switching doesn't cut");
        assert!(!cut(&mut cx), "the camera switched to cuts again");
        let flat = Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        };
        cx.world_mut()
            .get_mut::<Camera>(second)
            .expect("alive")
            .projection = flat;
        assert!(
            cut(&mut cx),
            "the projection turning orthographic doesn't cut"
        );
        assert!(!cut(&mut cx), "the orthographic camera cuts again");
        cx.set_viewpoint(Viewpoint::default());
        assert!(cut(&mut cx), "a layer setting a viewpoint doesn't cut");
        cx.set_viewpoint(Viewpoint {
            position: DVec3::X,
            ..Viewpoint::default()
        });
        assert!(!cut(&mut cx), "the layer's viewpoint moving cuts");
        assert!(
            cut(&mut cx),
            "the layer leaving it to the camera doesn't cut"
        );
    }

    /// The view is cut to the tool API's look as it's set, as each viewpoint it sets changes, and
    /// as it's cleared; and to and from a bare frame among the look's, which sees through the
    /// game's camera, but for a look without a viewpoint, whose bare frames see as the rest. Each
    /// frame says the look shows, for the exposure, as it sees from the look's viewpoint, so that
    /// every frame that turns to or from it is cut to.
    #[cfg(feature = "dev")]
    #[test]
    fn the_view_cuts_to_and_from_the_tools_look() {
        // Whether the next frame is cut to, and whether it says the look shows.
        let seen = |cx: &mut Context| {
            cx.draw_world(1.0, 0.0, 0);
            let mut frame = Extracted::default();
            let view = Viewport {
                origin: [0, 0],
                size: [64, 32],
            };
            cx.extract(&mut frame, view);
            (frame.cut, frame.looked)
        };
        let mut cx = Context::seeded(1);
        cx.world_mut().spawn(camera_at(0.0, true));
        seen(&mut cx);
        assert_eq!(seen(&mut cx), (false, false), "the camera");
        let looked = Viewpoint {
            position: DVec3::new(1.0, 2.0, 10.0),
            ..Viewpoint::default()
        };
        cx.look.viewpoint = Some(looked);
        assert_eq!(seen(&mut cx), (true, true), "the look set");
        assert_eq!(seen(&mut cx), (false, true), "the look held");
        cx.look.viewpoint = Some(Viewpoint {
            position: DVec3::ZERO,
            ..looked
        });
        assert_eq!(seen(&mut cx), (true, true), "the look changed");
        cx.look.bare = true;
        assert_eq!(
            seen(&mut cx),
            (true, false),
            "a bare frame among the look's"
        );
        cx.look.bare = false;
        assert_eq!(seen(&mut cx), (true, true), "the look's frame after it");
        cx.look.viewpoint = None;
        assert_eq!(seen(&mut cx), (true, false), "the look cleared");
        cx.look.bare = true;
        assert_eq!(seen(&mut cx), (false, false), "a bare frame without a look");
        cx.look.bare = false;
        assert_eq!(seen(&mut cx), (false, false), "the frame after it");
    }

    /// The passes the tool API's look leaves out are left out of the frame, each as it would be
    /// drawn were the game's environment without it, and its lights without shadows; a bare frame,
    /// a capture of the game's own view's, leaves none out; and the world, its environment and
    /// lights among it, stays as it was.
    #[cfg(feature = "dev")]
    #[test]
    fn the_passes_the_look_leaves_out_are_left_out_of_the_frame_but_a_bare_one() {
        use gg_renderer::{
            Adaptation, AmbientOcclusion, DepthOfField, Lens, MotionBlur, Reflections,
        };

        /// Each pass of a frame, by [`gg_api::Pass::ALL`]'s order, the shadows by the light's.
        type Passes = (
            Option<AmbientOcclusion>,
            Option<Reflections>,
            Option<f32>,
            Option<Fog>,
            f32,
            Option<DepthOfField>,
            Option<MotionBlur>,
            Antialiasing,
            Grade,
            Lens,
            Option<Adaptation>,
        );

        /// The passes of `frame`.
        fn passes(frame: &Extracted) -> Passes {
            (
                frame.ambient_occlusion,
                frame.reflections,
                frame.lights[0].shadow_distance,
                frame.fog,
                frame.bloom,
                frame.depth_of_field,
                frame.motion_blur,
                frame.antialiasing,
                frame.grade,
                frame.lens,
                frame.adaptation,
            )
        }
        let mut cx = Context::seeded(1);
        // Every pass on, none as a pass left out leaves it.
        let everything = Environment {
            adaptation: Some(crate::Adaptation::default()),
            bloom: 0.5,
            grade: Grade {
                contrast: 1.5,
                ..Grade::IDENTITY
            },
            fog: Some(Fog::default()),
            reflections: Some(Reflections::default()),
            depth_of_field: Some(crate::DepthOfField::default()),
            motion_blur: Some(crate::MotionBlur::default()),
            lens: crate::Lens {
                vignette: 0.4,
                ..crate::Lens::NONE
            },
            ..Environment::default()
        };
        let environment = cx.world_mut().spawn(everything);
        let light = (cx.world_mut()).spawn((Transform::IDENTITY, crate::Light::default()));
        let hash = cx.world().hash();
        let drawn = passes(&handed_off(&mut cx));
        let shadows = crate::Light::default().shadow_distance;
        let game = (
            everything.ambient_occlusion,
            everything.reflections,
            Some(shadows),
            everything.fog,
            everything.bloom,
            everything.depth_of_field.map(|focus| focus.rendered()),
            everything.motion_blur.map(|blur| blur.rendered()),
            Antialiasing::Taa,
            everything.grade,
            everything.lens.rendered(),
            everything
                .adaptation
                .map(|adaptation| adaptation.rendered()),
        );
        assert_eq!(drawn, game, "the game's own");
        cx.look.skip = gg_api::Pass::ALL.to_vec();
        let none = (
            None,
            None,
            None,
            None,
            0.0,
            None,
            None,
            Antialiasing::None,
            Grade::IDENTITY,
            Lens::NONE,
            None,
        );
        assert_eq!(passes(&handed_off(&mut cx)), none, "every pass left out");
        cx.look.skip = vec![gg_api::Pass::Reflections];
        let mut reflectionless = game;
        reflectionless.1 = None;
        assert_eq!(passes(&handed_off(&mut cx)), reflectionless, "one left out");
        cx.look.skip = gg_api::Pass::ALL.to_vec();
        cx.look.bare = true;
        assert_eq!(passes(&handed_off(&mut cx)), game, "a bare frame");
        let world = cx.world();
        assert_eq!(world.get::<Environment>(environment), Some(&everything));
        let lit = world.get::<crate::Light>(light).copied();
        assert_eq!(lit, Some(crate::Light::default()));
        assert_eq!(world.hash(), hash);
    }

    /// Each skinned part of a model's root draws a mesh of its own, a copy of its model's, which
    /// the frame skins by its skin's joints, each where it draws, from the mesh's space as it was
    /// bound: at rest, as the Fox spawns, the matrix of no change for each of its 24; the copy
    /// kept from frame to frame, and gone with its root.
    #[test]
    fn a_skinned_part_draws_a_mesh_of_its_own_moved_by_its_joints() {
        let mut cx = Context::seeded(1);
        let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/showcase/models/fox");
        let model = cx
            .add_model("fox", fox.join("Fox.gltf"))
            .expect("the Fox reads");
        let root = model.spawn(cx.world_mut(), Transform::IDENTITY);
        cx.draw_world(1.0, 0.0, 0);
        let model_mesh = cx.loaded.made[model.index()].meshes[0].index();
        let skins: Vec<(u32, Vec<Mat4>)> = (cx.draws.skins())
            .map(|(mesh, joints)| (mesh, joints.to_vec()))
            .collect();
        let [(own, joints)] = &skins[..] else {
            panic!("a skin: {skins:?}");
        };
        assert_ne!(*own, model_mesh, "a copy of its own");
        assert_eq!(joints.len(), 24);
        for (i, joint) in joints.iter().enumerate() {
            assert!(
                joint.abs_diff_eq(Mat4::IDENTITY, 1e-4),
                "joint {i} at rest: {joint}"
            );
        }
        let drawn: Vec<u32> = (cx.scene.changes().flatten())
            .filter_map(|(_, set)| set)
            .map(|(_, _, draw)| draw.parts().0)
            .collect();
        assert!(
            drawn.contains(own) && !drawn.contains(&model_mesh),
            "{drawn:?}"
        );
        cx.draws.clear();
        cx.draw_world(1.0, 0.0, 0);
        assert_eq!(
            cx.draws.skins().map(|(mesh, _)| mesh).collect::<Vec<_>>(),
            [*own]
        );
        assert_eq!(cx.skins.held(), (1, 0));
        cx.world_mut().despawn(root).expect("alive");
        cx.draws.clear();
        cx.draw_world(1.0, 0.0, 0);
        assert_eq!(cx.draws.skins().count(), 0);
        assert_eq!(cx.skins.held(), (0, 1), "its copy spare");
        // Another Fox takes the spare copy, which the renderer has, adding no mesh.
        let meshes = cx.meshes.count();
        model.spawn(cx.world_mut(), Transform::IDENTITY);
        cx.draws.clear();
        cx.draw_world(1.0, 0.0, 0);
        assert_eq!(
            cx.draws.skins().map(|(mesh, _)| mesh).collect::<Vec<_>>(),
            [*own]
        );
        assert_eq!((cx.meshes.count(), cx.skins.held()), (meshes, (1, 0)));
    }

    /// A Fox's context, the Fox's model added, after a frame of a Fox spawned and despawned, which
    /// leaves its skinned copy spare.
    fn spare_fox() -> (Context, Model) {
        let mut cx = Context::seeded(1);
        let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/showcase/models/fox");
        let model = cx
            .add_model("fox", fox.join("Fox.gltf"))
            .expect("the Fox reads");
        let root = model.spawn(cx.world_mut(), Transform::IDENTITY);
        cx.draw_world(1.0, 0.0, 0);
        cx.world_mut().despawn(root).expect("alive");
        cx.draws.clear();
        cx.draw_world(1.0, 0.0, 0);
        assert_eq!(cx.skins.held(), (0, 1));
        (cx, model)
    }

    /// A spare copy of a skinned model's mesh goes, and the renderer removes it, once its model's
    /// mesh is another, as the model read again makes it.
    #[test]
    fn a_spare_skinned_copy_goes_once_its_models_mesh_is_another() {
        let (mut cx, model) = spare_fox();
        let kept = cx.meshes.kept();
        cx.loaded.made[model.index()].skinned[0] = Some(Arc::new(
            cx.loaded.made[model.index()].skinned[0]
                .as_deref()
                .expect("a skinned mesh")
                .clone(),
        ));
        cx.draw_world(1.0, 0.0, 0);
        assert_eq!((cx.skins.held(), cx.meshes.kept()), ((0, 0), kept - 1));
    }

    /// A spare copy of a skinned model's mesh that no entity takes goes, and the renderer removes
    /// it, after `SPARE_FRAMES`, so that the copies of a crowd that went give back their memory.
    #[test]
    fn a_spare_skinned_copy_goes_once_its_time_is_up() {
        let (mut cx, _) = spare_fox();
        let kept = cx.meshes.kept();
        for _ in 1..crate::skins::SPARE_FRAMES {
            cx.draws.clear();
            cx.draw_world(1.0, 0.0, 0);
        }
        assert_eq!((cx.skins.held(), cx.meshes.kept()), ((0, 1), kept));
        cx.draw_world(1.0, 0.0, 0);
        assert_eq!((cx.skins.held(), cx.meshes.kept()), ((0, 0), kept - 1));
    }

    /// A skinned part's copy whose entity is alive, and no longer skinned, as its draw was changed
    /// to another mesh, goes rather than being spare, as the scene may still draw it: the next Fox
    /// has a copy of its own.
    #[test]
    fn a_skinned_copy_of_an_entity_still_alive_goes_rather_than_being_spare() {
        let mut cx = Context::seeded(1);
        let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/showcase/models/fox");
        let model = cx
            .add_model("fox", fox.join("Fox.gltf"))
            .expect("the Fox reads");
        model.spawn(cx.world_mut(), Transform::IDENTITY);
        cx.draw_world(1.0, 0.0, 0);
        let model_mesh = cx.loaded.made[model.index()].meshes[0].index();
        let (part, draw) = (cx.world().query::<(Entity, &MeshDraw)>())
            .find(|(_, draw)| draw.parts().0 == model_mesh)
            .map(|(part, draw)| (part, *draw))
            .expect("the skinned part");
        let (_, shader, opaque, culls, params) = draw.parts();
        let quad = MeshDraw::from_parts(0, shader, opaque, culls, params).expect("a draw");
        let [(own, _)] = cx
            .draws
            .skins()
            .map(|(mesh, _)| (mesh, ()))
            .collect::<Vec<_>>()[..]
        else {
            panic!("a skin");
        };
        let kept = cx.meshes.kept();
        cx.world_mut().insert(part, quad).expect("alive");
        cx.draws.clear();
        cx.draw_world(1.0, 0.0, 0);
        assert_eq!((cx.skins.held(), cx.meshes.kept()), ((0, 0), kept - 1));
        model.spawn(cx.world_mut(), Transform::IDENTITY);
        cx.draws.clear();
        cx.draw_world(1.0, 0.0, 0);
        let skins: Vec<u32> = cx.draws.skins().map(|(mesh, _)| mesh).collect();
        assert!(
            skins.len() == 1 && skins[0] != own,
            "{skins:?} beside {own}"
        );
        assert_eq!(cx.meshes.kept(), kept);
    }

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

    /// What the app keeps for the user reads back as it was saved, from the next run's context
    /// too, a file not saved reading as none; a name that isn't a file's alone is refused, saving
    /// nothing; and a headless run's context reads none and keeps nothing.
    #[test]
    fn the_apps_state_reads_back_as_it_was_kept() {
        let scratch = crate::dirs::tests::Scratch::new("app-state");
        let mut cx = Context::new(None);
        cx.state = Some(scratch.0.clone());
        assert_eq!(cx.read_state("projects.json").expect("read"), None);
        cx.save_state("projects.json", b"first").expect("kept");
        cx.save_state("projects.json", b"second")
            .expect("kept again");
        let mut next = Context::new(None);
        next.state = Some(scratch.0.clone());
        let read = next.read_state("projects.json").expect("read");
        assert_eq!(read.as_deref(), Some(&b"second"[..]));
        for name in ["", "..", "nested/projects.json", "../projects.json"] {
            let refused = cx.save_state(name, b"x").expect_err(name);
            assert_eq!(refused.code(), "engine.state", "{name:?}");
            assert_eq!(
                cx.read_state(name).err().map(|e| e.code()),
                Some("engine.state")
            );
        }
        let headless = Context::new(None);
        headless
            .save_state("projects.json", b"x")
            .expect("nothing kept");
        assert_eq!(headless.read_state("projects.json").expect("read"), None);
        let kept = std::fs::read_dir(&scratch.0).expect("listed").count();
        assert_eq!(kept, 1, "projects.json alone");
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

    /// Fails as it attaches.
    struct Failer;

    impl Layer for Failer {
        fn attach(&mut self, cx: &mut Context) {
            cx.fail(Error::new("test.failed", "it can't go on"));
        }
    }

    /// A layer that fails ends the run, as an exit does, logging its error by its code at once; the
    /// layers still detach, after which the stack says the run failed, as one that exits alone
    /// doesn't.
    #[test]
    fn a_layer_that_fails_ends_the_run_with_its_error() {
        let logged = crate::testing::logged();
        let (mut stack, calls) = probes(false);
        stack.push_layer(Failer);
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        assert!(cx.exiting());
        let codes = logged
            .codes
            .try_iter()
            .filter(|code| code.starts_with("test."));
        assert_eq!(codes.collect::<Vec<_>>(), ["test.failed"]);
        stack.detach(&mut cx);
        assert!(stack.failed());
        let detached = calls
            .borrow()
            .iter()
            .filter(|call| call.starts_with("detach"))
            .count();
        assert_eq!(detached, 3);

        let (mut stack, _) = probes(false);
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        cx.exit();
        stack.detach(&mut cx);
        assert!(cx.exiting() && !stack.failed());
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
    #[cfg(feature = "dev")]
    struct Quiet;

    #[cfg(feature = "dev")]
    impl Layer for Quiet {}

    /// Draws nothing, through a `ui` of its own.
    #[cfg(feature = "dev")]
    struct Blank;

    #[cfg(feature = "dev")]
    impl Layer for Blank {
        fn ui(&mut self, _: &mut Context, _: &mut Ui<'_>) {}
    }

    /// Draws what the layer it wraps does, through a `ui` of its own.
    #[cfg(feature = "dev")]
    struct Wrapper(Quiet);

    #[cfg(feature = "dev")]
    impl Layer for Wrapper {
        fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
            self.0.ui(cx, ui);
        }
    }

    /// Whether `stack`'s layers draw UI, as a pass of egui's tells.
    #[cfg(feature = "dev")]
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
    #[cfg(feature = "dev")]
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

    /// A run that reads a package loads from it what layers add and text names, though no file of
    /// theirs is on the disk: a texture, compressed, by its path, however it's written, named by
    /// its ID, and by that ID, as text names it; a model, cooked, its primitives with levels of
    /// detail and its images compressed; a scene's text; a script; and a prefab, by its ID.
    #[test]
    fn a_run_loads_from_its_package_with_no_file_on_the_disk() {
        use crate::image::tests::ktx2;
        use crate::package::{Kind, Package, Writer};

        let dir = crate::dirs::tests::Scratch::new("package-loads");
        // Never made: what loads, loads from the package.
        let root = dir.0.join("game");
        // A block of BC7, in KTX2.
        let block = |size| ktx2(ktx2::Format::BC7_SRGB_BLOCK, size, &[vec![0; 16]], true);
        // The glTF tests' model, cooked: each primitive's triangles twice, its second level, and
        // each image a block.
        let glb = crate::gltf::tests::model();
        let read = |_: &Path, _: u64| Ok(glb.clone());
        let mut model = (crate::gltf::read("m", Path::new("m.glb"), read, u64::MAX))
            .unwrap_or_else(|error| panic!("{error:#}"));
        for primitive in &mut model.primitives {
            let count = primitive.indices.len() as u32;
            primitive.indices.extend_from_within(..);
            let levels = vec![
                gg_renderer::Lod {
                    start: 0,
                    count,
                    error: 0.0,
                },
                gg_renderer::Lod {
                    start: count,
                    count,
                    error: 0.5,
                },
            ];
            let mut mesh = crate::model::model_mesh("m", primitive).expect("a mesh");
            mesh.lods.levels = levels;
            primitive.mesh = Some(mesh);
        }
        // Each compressed as what it holds says, as the cooker compresses it.
        let images: Vec<_> = (model.images.iter().enumerate())
            .map(|(i, image)| {
                let holds = crate::model::texels(&model.primitives, i);
                let format = crate::image::Compressed::of(holds).ktx2();
                let size = image.size.map(|side| side.min(4));
                ktx2(format, size, &[vec![0; 16]], true)
            })
            .collect();
        let still = "local Still = gg.declare(\"Still\", { rate = 1 })\nreturn Still\n";
        let mut writer = Writer::new(".", Some("scenes/main.ron"), None);
        let id = 0x5f3c_9a1e_07b2_d846;
        writer
            .add("textures/stone.png", Some(id), Kind::Texture, block([1, 1]))
            .unwrap();
        let cooked = crate::cooked::encode(&model, &images);
        writer
            .add("models/m.glb", None, Kind::Model, cooked)
            .unwrap();
        let scene = b"(\n    entities: {},\n)\n".to_vec();
        writer
            .add("scenes/main.ron", None, Kind::File, scene.clone())
            .unwrap();
        writer
            .add("scripts/still.luau", None, Kind::File, still.into())
            .unwrap();
        let lamp = b"(\n    entities: {\n        \"0000000000000001\": (\n            Sprite: (\n                \
                     texture: Some(\"5f3c9a1e07b2d846\"),\n            ),\n        ),\n    },\n)\n";
        let lamp_id = 0x6b5e_1c3f_29d0_a487;
        writer
            .add(
                "prefabs/lamp.prefab",
                Some(lamp_id),
                Kind::File,
                lamp.to_vec(),
            )
            .unwrap();
        let path = dir.0.join("game.ggpk");
        writer.write(&path).unwrap();
        let mut cx = Context::seeded(1);
        cx.root = Some(root.clone());
        cx.files = Files::package(Arc::new(Package::open(&path).unwrap()), root.clone());
        assert!(cx.reads_package());

        let stone = (cx.add_texture("stone", root.join("textures/stone.png")))
            .unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(
            cx.assets.name(stone),
            Some("5f3c9a1e07b2d846"),
            "named by its ID"
        );
        assert_eq!(cx.assets.path(stone), Some("textures/stone.png"));
        let again = cx.add_texture("again", root.join("textures/../textures/stone.png"));
        assert_eq!(again.expect("the same"), stone);
        let gone = cx.add_texture("gone", root.join("textures/gone.png"));
        assert_eq!(
            gone.expect_err("not in the package").code(),
            "engine.bad_texture"
        );
        let (meshes, textures) = (cx.meshes.count(), cx.textures.count());
        (cx.add_model("m", root.join("models/m.glb"))).unwrap_or_else(|error| panic!("{error:#}"));
        let added = (cx.meshes.count() - meshes, cx.textures.count() - textures);
        let counts = (model.primitives.len() as u32, model.images.len() as u32);
        assert_eq!(
            added, counts,
            "a mesh for each primitive, a texture for each image"
        );
        // As text names it, by its ID, which a run without `dev` finds in the package alone.
        cx.load_named([Asset::Texture("5f3c9a1e07b2d846")], false);
        assert_eq!(cx.assets.texture("5f3c9a1e07b2d846"), Some(stone));
        let unknown = cx.asset_path("0123456789abcdef", "texture", false);
        assert_eq!(unknown.expect_err("no such ID").code(), "asset.unknown_id");

        let read = crate::ecs::read_scene_from(&root.join("scenes/main.ron"), &cx.files);
        assert_eq!(read.expect("the scene's text").as_bytes(), scene);
        let mut scripts = crate::Scripts::new().expect("Luau starts");
        let declared = scripts.load(&mut cx, root.join("scripts/still.luau"));
        assert_eq!(declared.expect("the script loads"), "Still");
        // A prefab by its ID, whose instance's sprite shows the texture it names.
        cx.load_named([Asset::Prefab("6b5e1c3f29d0a487")], false);
        let prefab = cx.assets.prefab("6b5e1c3f29d0a487").expect("by its ID");
        assert_eq!(cx.assets.prefab_path(prefab), "prefabs/lamp.prefab");
        let placed = prefab.spawn(cx.world_mut(), Transform::IDENTITY);
        let placed = placed.unwrap_or_else(|error| panic!("{error:#}"));
        let sprite = cx.world().get::<Sprite>(placed).expect("the prefab's");
        assert_eq!(sprite.texture, Some(stone));
        // Nor does a scan, as the tool API's `assets` asks for, read the disk under the root, or
        // write an ID file there.
        #[cfg(feature = "dev")]
        {
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("unlisted.png"), b"").unwrap();
            cx.scan_assets(true);
            assert!(
                !root.join("unlisted.png.meta").exists(),
                "an ID file written"
            );
        }
    }

    #[test]
    fn a_sprite_sheet_loads_only_if_its_cells_fit_its_image() {
        let (mut loader, mut textures) = (Loader::inline(), Textures::default());
        let path = Path::new("assets/tiles.png");
        let image = || Image {
            size: [64, 32],
            pixels: vec![0; 64 * 32 * 4],
            format: image::Format::Rgba8,
        };
        for cell in [[0, 16], [16, 0], [65, 16], [16, 33]] {
            let error = add_sheet(&mut loader, &mut textures, "tiles", path, image(), cell)
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
        let sheet = add_sheet(&mut loader, &mut textures, "tiles", path, image(), [64, 32]);
        let sheet = sheet.expect("a sheet");
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
        let named: Vec<_> = assets.named_from((0, 0, 0, 0)).collect();
        let images = &named[1..named.len() - 1];
        assert_eq!(named[0], (stone, dotted));
        assert!(!images.is_empty(), "{named:?}");
        // The model itself, after the textures.
        assert_eq!(named.last(), Some(&(model, "assets/m.glb")));
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

    /// A texture's file renamed with its ID file while the run has it loaded, added again by its
    /// new path once the database has it there, gives the texture loaded before, as by its ID,
    /// loading nothing again; a sheet cut from it too.
    #[cfg(feature = "dev")]
    #[test]
    fn a_texture_renamed_while_loaded_loads_once_by_its_new_path() {
        use gg_api::asset::Meta;

        let scratch = crate::dirs::tests::Scratch::new("renamed-loaded");
        let root = scratch.0.clone();
        let assets = root.join("assets");
        std::fs::create_dir_all(&assets).expect("a directory");
        let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 2, &[9; 8], None);
        std::fs::write(assets.join("stone.png"), png).expect("written");
        let meta = Meta {
            id: "5f3c9a1e07b2d846".parse().expect("an ID"),
        };
        std::fs::write(assets.join("stone.png.meta"), meta.text()).expect("written");
        let mut cx = Context::seeded(0).root(root.clone());
        let stone = cx.add_texture("stone", assets.join("stone.png"));
        let stone = stone.expect("a texture");
        for name in ["stone.png", "stone.png.meta"] {
            let to = assets.join(name.replacen("stone", "rock", 1));
            std::fs::rename(assets.join(name), to).expect("renamed");
        }
        cx.scan_assets(false);
        let count = cx.assets.counts();
        let rock = cx.add_texture("rock", assets.join("rock.png"));
        assert_eq!(rock.expect("a texture"), stone);
        let sheet = cx.add_sprite_sheet("rock", assets.join("rock.png"), [1, 1]);
        assert_eq!(sheet.expect("a sheet").texture, stone);
        assert_eq!(cx.assets.counts(), count, "nothing loaded again");
        assert_eq!(cx.assets.texture("assets/rock.png"), Some(stone));
    }

    /// A run that serves the tool API and writes ID files keeps the prints of what its assets hold
    /// in its cache, by the thread that watches them, so that the next run's start gives an asset
    /// renamed alone while no run watched the ID it had, by the ID file the rename left behind; a
    /// run that writes none reads no cache, and moves nothing.
    #[cfg(feature = "dev")]
    #[test]
    fn a_runs_start_keeps_the_id_of_an_asset_renamed_while_none_watched() {
        let scratch = crate::dirs::tests::Scratch::new("start-renamed");
        let root = scratch.0.join("game");
        std::fs::create_dir_all(&root).expect("a directory");
        std::fs::write(root.join("a.png"), "alpha").expect("an asset");
        let cache = scratch.0.join("cache").join("prints.json");
        let started = |serves, writes| {
            let mut cx = Context::seeded(1);
            cx.root = Some(root.clone());
            cx.asset_cache = Some(cache.clone());
            cx.start_assets(serves, writes);
            cx
        };
        let cx = started(true, true);
        let a = cx.catalog().id("a.png").expect("an ID written");
        cx.ask_assets().expect("a thread watches");
        // The thread keeps the prints after it answers a look, and a run's end waits for it no
        // longer than `asset_watch::STOP`: until the cache holds a.png's.
        let kept =
            || std::fs::read_to_string(&cache).is_ok_and(|kept| kept.contains(&a.to_string()));
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while !kept() {
            assert!(
                std::time::Instant::now() < until,
                "the thread kept the prints"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(cx);
        std::fs::rename(root.join("a.png"), root.join("b.png")).expect("renamed");
        let reads = started(false, false);
        assert_eq!(reads.catalog().id("b.png"), None, "a run that writes none");
        assert!(root.join("a.png.meta").exists());
        let writes = started(false, true);
        assert_eq!(writes.catalog().id("b.png"), Some(a));
        assert!(!writes.watches_assets(), "a run that serves no API");
        assert!(!root.join("a.png.meta").exists(), "moved beside b.png");
    }

    /// In a run that reloads assets, a texture whose file is edited takes the image as a frame
    /// starts, under its handle, at another size; one renamed with its ID file is followed where the
    /// database finds it, and an edit there taken; a stand-in takes its file's image once the file
    /// comes, which a later add of the file gives, and is followed by the ID the database gives it
    /// then; a sprite sheet's texture keeps its size, warning, since its cells would move; and a
    /// stand-in for a file outside the asset root takes none of it.
    #[cfg(feature = "dev")]
    #[test]
    fn a_texture_edited_renamed_or_come_reloads_under_its_handle() {
        use gg_api::asset::Meta;

        let scratch = crate::dirs::tests::Scratch::new("reloaded");
        let root = scratch.0.join("game");
        let assets = root.join("assets");
        std::fs::create_dir_all(&assets).expect("a directory");
        let png = |width: u32| {
            let pixels = vec![9; width as usize * 4];
            image::tests::png(
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                width,
                &pixels,
                None,
            )
        };
        // Dated an hour back, so that each loads as it is.
        let written_long_ago = |name: &str, bytes: &[u8]| {
            let path = assets.join(name);
            std::fs::write(&path, bytes).expect("written");
            let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
            let file = std::fs::File::options().write(true).open(path);
            (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
        };
        written_long_ago("stone.png", &png(1));
        let meta = Meta {
            id: "5f3c9a1e07b2d846".parse().expect("an ID"),
        };
        written_long_ago("stone.png.meta", meta.text().as_bytes());
        written_long_ago("sheet.png", &png(2));
        let logged = crate::testing::logged();
        let mut cx = Context::seeded(0).root(root.clone());
        cx.hot_reload = Some(HotReload::start().expect("the thread"));
        let stone = cx.add_texture("stone", assets.join("stone.png"));
        let stone = stone.expect("a texture");
        let sheet = cx.add_sprite_sheet("sheet", assets.join("sheet.png"), [1, 1]);
        let sheet = sheet.expect("a sheet").texture;
        cx.load_named([Asset::Texture("assets/later.png")], true);
        let later = cx.assets.texture("assets/later.png").expect("a stand-in");

        let size = |cx: &Context, texture: Texture| {
            let loaded = cx.loaded.textures.values();
            loaded
                .copied()
                .find(|&(loaded, _)| loaded == texture)
                .map(|(_, size)| size)
        };
        // Frame starts until `done`.
        let until = |cx: &mut Context, done: &dyn Fn(&Context) -> bool, what: &str| {
            let until = std::time::Instant::now() + Duration::from_secs(30);
            while !done(cx) {
                assert!(std::time::Instant::now() < until, "{what}");
                std::thread::sleep(Duration::from_millis(5));
                cx.take_reloaded(false);
            }
        };
        std::fs::write(assets.join("stone.png"), png(3)).expect("an edit");
        until(
            &mut cx,
            &|cx| size(cx, stone) == Some([3, 1]),
            "the edit reloads",
        );

        // Renamed with its ID file, and edited at once, before the database has it there.
        for name in ["stone.png", "stone.png.meta"] {
            let to = assets.join(name.replacen("stone", "rock", 1));
            std::fs::rename(assets.join(name), to).expect("renamed");
        }
        std::fs::write(assets.join("rock.png"), png(4)).expect("an edit");
        cx.scan_assets(false);
        until(
            &mut cx,
            &|cx| size(cx, stone) == Some([4, 1]),
            "the edit where it is reloads",
        );

        std::fs::write(assets.join("later.png"), png(5)).expect("its file comes");
        until(
            &mut cx,
            &|cx| size(cx, later) == Some([5, 1]),
            "the stand-in gives way",
        );
        let added = cx.add_texture("later", assets.join("later.png"));
        assert_eq!(added.expect("a texture"), later, "loaded once");
        // Given an ID file, which the database reads, then renamed with it and edited at once.
        let meta = Meta {
            id: "7a1b2c3d4f5a6b7c".parse().expect("an ID"),
        };
        std::fs::write(assets.join("later.png.meta"), meta.text()).expect("written");
        cx.scan_assets(false);
        for name in ["later.png", "later.png.meta"] {
            let to = assets.join(name.replacen("later", "other", 1));
            std::fs::rename(assets.join(name), to).expect("renamed");
        }
        std::fs::write(assets.join("other.png"), png(6)).expect("an edit");
        cx.scan_assets(false);
        until(
            &mut cx,
            &|cx| size(cx, later) == Some([6, 1]),
            "the edit where the stand-in's file is now reloads",
        );

        let warned = |code: &'static str| {
            let logged = &logged;
            move |_: &Context| logged.codes.try_iter().any(|logged| logged == code)
        };
        std::fs::write(assets.join("sheet.png"), png(7)).expect("an edit");
        let resized = warned("asset.sheet_resized");
        until(&mut cx, &resized, "a sheet's texture keeps its size");
        assert_eq!(size(&cx, sheet), Some([2, 1]));

        // Outside the root, through `..`: one there is refused, and isn't watched; one that comes
        // after is refused as it comes.
        std::fs::write(scratch.0.join("there.png"), png(8)).expect("written");
        cx.load_named(["../there.png", "../after.png"].map(Asset::Texture), true);
        let there = cx.assets.texture("../there.png").expect("a stand-in");
        let after = cx.assets.texture("../after.png").expect("a stand-in");
        std::fs::write(scratch.0.join("there.png"), png(9)).expect("an edit");
        std::fs::write(scratch.0.join("after.png"), png(10)).expect("it comes");
        let outside = warned("asset.outside_root");
        until(&mut cx, &outside, "a file outside the root is refused");
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(10));
            cx.take_reloaded(false);
        }
        assert_eq!((size(&cx, there), size(&cx, after)), (None, None));
    }

    /// An ID no asset has, which the project's fetch list gives a file, names that file and `cargo
    /// gg fetch`, the list read beside the manifest, not under the asset root; but not an ID the
    /// list doesn't give, nor in a context that isn't a project's game.
    #[cfg(feature = "dev")]
    #[test]
    fn an_unknown_id_the_fetch_list_gives_names_what_fetches_it() {
        let scratch = crate::dirs::tests::Scratch::new("unknown-fetched");
        let dir = scratch.0.join("game");
        std::fs::create_dir_all(dir.join("content")).expect("a directory");
        let project = gg_api::Project {
            game: "tested".to_owned(),
            assets: "content".to_owned(),
            scenes: vec!["content/scenes".to_owned()],
            start: "content/scenes/main.ron".to_owned(),
            relay: None,
            icon: None,
        };
        std::fs::write(dir.join(gg_api::Project::FILE), project.text()).expect("written");
        let (id, hash) = (
            "44ae3d1eca56e109",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
        let list = format!(
            "[\n    (\n        from: \"https://example.com/sponza\",\n        into: \
             \"content/models/sponza\",\n        files: {{\n            \"Sponza.gltf\": \
             \"{hash}\",\n        }},\n        ids: {{\n            \"Sponza.gltf\": \
             \"{id}\",\n        }},\n    ),\n]\n"
        );
        std::fs::write(dir.join(gg_api::Fetch::FILE), list).expect("written");

        let cx = Context::seeded(0).project(&dir).expect("the project reads");
        let error = cx.asset_path(id, "model", false).expect_err("not fetched");
        let message = format!("{error:#}");
        assert_eq!(error.code(), "asset.unknown_id", "{message}");
        assert!(
            message.contains("`content/models/sponza/Sponza.gltf`")
                && message.contains("`cargo gg fetch`"),
            "{message}"
        );
        let other = cx.asset_path("0a1b2c3d4e5f6071", "model", false);
        let message = format!("{:#}", other.expect_err("no such ID"));
        assert!(!message.contains("fetch"), "{message}");

        let rooted = Context::seeded(0).root(dir.join("content"));
        let message = format!(
            "{:#}",
            rooted.asset_path(id, "model", false).expect_err("none")
        );
        assert!(!message.contains("fetch"), "{message}");
    }

    /// A scene file loads the textures it names that no layer added, by an ID or by a path from the
    /// root, once each, named as a layer's add would name them, and a model's image with its model.
    /// One naming a texture that can't be loaded is refused where it names it, saying why,
    /// spawning nothing: a file that isn't an image or isn't there, one outside the root, by an
    /// absolute path or through `..`, an ID no asset has, and an image a model hasn't. A run that
    /// follows another loads them, and stands in for those, named as named; and asked by the tool
    /// API, as a lockstep joiner, loads only what it was given.
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
        let image = format!("{model}#0");
        let path = scene(
            0x1000,
            &[stone, "assets/plain.png", "assets/./stone.png", &image],
        );
        cx.load_scene_file(&path).expect("loaded");
        assert_eq!(
            cx.textures.count(),
            count + 4,
            "each once, the model's two images among them"
        );
        assert!(
            cx.assets.model(model).is_some(),
            "the model, with its image"
        );
        let mut names = shown(&cx);
        names.sort_unstable();
        assert_eq!(names, [&image, stone, stone, "assets/plain.png"]);

        let absolute = root
            .join("assets/plain.png")
            .to_string_lossy()
            .replace('\\', "/");
        let image = format!("{model}#9");
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
        joiner.load_named([stone, "assets/plain.png"].map(Asset::Texture), false);
        assert!(joiner.assets.texture(stone).is_some());
        let plain = joiner.assets.not_found("texture", "assets/plain.png");
        assert!(plain.contains("asset.not_given"), "{plain}");
    }

    /// A scene that names a model by its ID loads it, and its root spawns the model's nodes. Edited,
    /// the model's nodes and materials reload, the root's nodes taking them, unless the run is
    /// recorded, which keeps them, warning; and renamed with its ID file, the model loads by its
    /// ID from where it is.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_names_a_model_by_its_id_whose_nodes_reload_as_it_changes() {
        use gg_api::asset::Meta;

        let logged = crate::testing::logged();
        let scratch = crate::dirs::tests::Scratch::new("model-by-id");
        let root = scratch.0.join("game");
        for dir in ["assets", "scenes"] {
            std::fs::create_dir_all(root.join(dir)).expect("a directory");
        }
        let id = "3c4d5e6f708192a3";
        let gltf = |translation: [f32; 3], red: f32| {
            gltf::tests::triangle_gltf(|gltf| {
                gltf["nodes"][0]["translation"] = serde_json::json!(translation);
                let color = serde_json::json!([red, 1.0, 1.0, 1.0]);
                gltf["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"] = color;
            })
        };
        // Dated an hour back, so that it loads as it is.
        let written_long_ago = |name: &str, bytes: &[u8]| {
            let path = root.join("assets").join(name);
            std::fs::write(&path, bytes).expect("written");
            let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
            let file = std::fs::File::options().write(true).open(path);
            (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
        };
        written_long_ago("tri.gltf", gltf([0.0; 3], 1.0).as_bytes());
        let meta = Meta {
            id: id.parse().expect("an ID"),
        };
        written_long_ago("tri.gltf.meta", meta.text().as_bytes());
        let scene = root.join("scenes/s.ron");
        let text = format!(
            "(\n    entities: {{\n        \"0000000000000100\": (\n            ModelRoot: \
             (\"{id}\"),\n            Transform: (),\n        ),\n    }},\n)\n"
        );
        std::fs::write(&scene, text).expect("written");

        let mut cx = Context::seeded(0).root(root.clone());
        cx.hot_reload = Some(HotReload::start().expect("the thread"));
        cx.load_scene_file(&scene).expect("loaded");
        let model = cx.assets.model(id).expect("loaded by its ID");
        assert_eq!(cx.assets.model_path(model), "assets/tri.gltf");
        // Its image, named through another path to its file.
        let image = "assets/./tri.gltf#0";
        cx.load_named([Asset::Texture(image)], false);
        let found = cx.assets.texture(image);
        assert!(found.is_some() && found == cx.assets.texture(&format!("{id}#0")));
        let placed = cx
            .world()
            .entity("0000000000000100".parse().expect("an ID"));
        let placed = placed.expect("its root");
        let node = |cx: &Context| -> Entity {
            let mut children = cx.world().children(placed).expect("alive");
            children.next().expect("its node")
        };
        let at = |cx: &Context| {
            cx.world()
                .get::<Transform>(node(cx))
                .expect("a node")
                .translation
        };
        let draw = |cx: &Context| *cx.world().get::<MeshDraw>(node(cx)).expect("a draw");
        assert_eq!(cx.world().get::<ModelNode>(node(&cx)), Some(&ModelNode(0)));
        assert_eq!(at(&cx), DVec3::ZERO);
        let drew = draw(&cx);

        // Frame starts, as a run that's `recorded` or not, until `done`.
        let until = |cx: &mut Context, recorded, done: &dyn Fn(&Context) -> bool, what: &str| {
            let until = std::time::Instant::now() + Duration::from_secs(30);
            while !done(cx) {
                assert!(std::time::Instant::now() < until, "{what}");
                std::thread::sleep(Duration::from_millis(5));
                cx.take_reloaded(recorded);
            }
        };
        let path = root.join("assets/tri.gltf");
        std::fs::write(&path, gltf([0.0, 5.0, 0.0], 0.5)).expect("an edit");
        let moved = |cx: &Context| at(cx) == DVec3::new(0.0, 5.0, 0.0);
        until(&mut cx, false, &moved, "the node moves");
        // Its material's color changed in the table of materials, where its draw reads it.
        assert_eq!(draw(&cx), drew, "the same draw, of the same material");

        let _ = logged.codes.try_iter().count();
        std::fs::write(&path, gltf([0.0, 7.0, 0.0], 0.5)).expect("an edit");
        let warned = |_: &Context| {
            logged
                .codes
                .try_iter()
                .any(|code| code == "asset.model_changed")
        };
        until(&mut cx, true, &warned, "a recorded run warns");
        assert!(moved(&cx), "and keeps its nodes");

        drop(cx);
        for name in ["tri.gltf", "tri.gltf.meta"] {
            let to = root.join("assets").join(name.replacen("tri", "moved", 1));
            std::fs::rename(root.join("assets").join(name), to).expect("renamed");
        }
        let mut cx = Context::seeded(0).root(root.clone());
        cx.load_scene_file(&scene).expect("loaded");
        let model = cx.assets.model(id).expect("loaded by its ID");
        assert_eq!(cx.assets.model_path(model), "assets/moved.gltf");
        assert!(cx.world().children(placed).expect("alive").next().is_some());
    }

    /// A scene that names a prefab by its ID loads it, and the textures its text names, and its
    /// instance spawns the prefab's entities. Edited, the prefab reloads, and its instance takes
    /// what changed but what it changed itself, unless the run is recorded, which keeps it,
    /// warning; and a run following another stands in for a prefab it can't load, whose instances
    /// are their roots alone.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_names_a_prefab_by_its_id_whose_instances_reload_as_it_changes() {
        use gg_api::asset::Meta;

        let logged = crate::testing::logged();
        let scratch = crate::dirs::tests::Scratch::new("prefab-by-id");
        let root = scratch.0.join("game");
        for dir in ["assets", "scenes"] {
            std::fs::create_dir_all(root.join(dir)).expect("a directory");
        }
        let id = "4d5e6f708192a3b4";
        let text_of = |scale: f32, name: &str| {
            format!(
                "(\n    entities: {{\n        \"0000000000000001\": (\n            Name: \
                 (\"{name}\"),\n            Sprite: (\n                color: (1.0, 1.0, 1.0, \
                 1.0),\n                texture: Some(\"assets/stone.png\"),\n            \
                 ),\n        ),\n        \"0000000000000002\": (\n            Parent: \
                 (\"0000000000000001\"),\n            Transform: (\n                scale: \
                 ({scale}, {scale}, 1.0),\n            ),\n        ),\n    }},\n)\n"
            )
        };
        // Dated an hour back, so that it loads as it is.
        let written_long_ago = |name: &str, bytes: &[u8]| {
            let path = root.join("assets").join(name);
            std::fs::write(&path, bytes).expect("written");
            let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
            let file = std::fs::File::options().write(true).open(path);
            (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
        };
        written_long_ago("lamp.prefab", text_of(2.0, "lamp").as_bytes());
        let meta = Meta {
            id: id.parse().expect("an ID"),
        };
        written_long_ago("lamp.prefab.meta", meta.text().as_bytes());
        let (color, depth) = (png::ColorType::Rgba, png::BitDepth::Eight);
        let png = crate::image::tests::png(color, depth, 1, &[255; 4], None);
        written_long_ago("stone.png", &png);
        let scene = root.join("scenes/s.ron");
        let text = format!(
            "(\n    entities: {{\n        \"0000000000000100\": (\n            Name: \
             (\"mine\"),\n            PrefabRoot: (\"{id}\"),\n        ),\n    }},\n)\n"
        );
        std::fs::write(&scene, &text).expect("written");

        let mut cx = Context::seeded(0).root(root.clone());
        cx.hot_reload = Some(HotReload::start().expect("the thread"));
        cx.load_scene_file(&scene).expect("loaded");
        let prefab = cx.assets.prefab(id).expect("loaded by its ID");
        assert_eq!(cx.assets.prefab_path(prefab), "assets/lamp.prefab");
        assert!(
            cx.assets.texture("assets/stone.png").is_some(),
            "what it names"
        );
        let placed = cx
            .world()
            .entity("0000000000000100".parse().expect("an ID"));
        let placed = placed.expect("its root");
        let child = |cx: &Context| -> Entity {
            let mut children = cx.world().children(placed).expect("alive");
            children.next().expect("its child")
        };
        let scale = |cx: &Context| {
            let transform = cx.world().get::<Transform>(child(cx));
            transform.expect("the prefab's").scale
        };
        assert_eq!(scale(&cx), Vec3::new(2.0, 2.0, 1.0));
        let name = |cx: &Context| cx.world().get::<Name>(placed).cloned();
        assert_eq!(name(&cx), Some(Name("mine".to_owned())), "its own");

        // Frame starts, as a run that's `recorded` or not, until `done`.
        let until = |cx: &mut Context, recorded, done: &dyn Fn(&Context) -> bool, what: &str| {
            let until = std::time::Instant::now() + Duration::from_secs(30);
            while !done(cx) {
                assert!(std::time::Instant::now() < until, "{what}");
                std::thread::sleep(Duration::from_millis(5));
                cx.take_reloaded(recorded);
            }
        };
        let path = root.join("assets/lamp.prefab");
        std::fs::write(&path, text_of(3.0, "post")).expect("an edit");
        let grown = |cx: &Context| scale(cx) == Vec3::new(3.0, 3.0, 1.0);
        until(&mut cx, false, &grown, "the instance takes it");
        assert_eq!(
            name(&cx),
            Some(Name("mine".to_owned())),
            "and keeps its own"
        );

        let _ = logged.codes.try_iter().count();
        std::fs::write(&path, text_of(4.0, "post")).expect("an edit");
        let warned = |_: &Context| {
            logged
                .codes
                .try_iter()
                .any(|code| code == "asset.prefab_changed")
        };
        until(&mut cx, true, &warned, "a recorded run warns");
        assert!(grown(&cx), "and keeps its instances");

        let mut follower = Context::seeded(0);
        follower.follows = true;
        follower.load_named([Asset::Prefab("assets/none.prefab")], true);
        let stand_in = follower
            .assets
            .prefab("assets/none.prefab")
            .expect("a stand-in");
        assert_eq!(follower.assets.prefab_text(stand_in), None);
        let world = follower.world_mut();
        let root = stand_in.spawn(world, Transform::IDENTITY).expect("spawned");
        assert_eq!(
            world.children(root).expect("alive").count(),
            0,
            "its root alone"
        );
    }

    /// A UI added by its file's asset ID is named by its ID and loads the texture it names, its
    /// image fit to the texture's size; its layer sets a text by its name, refused for a name no
    /// node has or a node that says nothing, and moves its focus by a key; a second add of its
    /// file reads it no more, sharing it, and is a UI of its own. Edited, the file reads again, and
    /// each UI takes its nodes, keeping the text code set and the focus by their names; an edit
    /// naming a texture that doesn't load warns with its code, and the UIs stay as they were.
    /// Renamed with its ID file, the file is found by its ID at its new path, which each UI lists,
    /// a third add by that path sharing it, and its edits there read again; and a new file at its
    /// old path is a file of its own.
    #[cfg(feature = "dev")]
    #[test]
    fn a_ui_added_by_its_id_reloads_as_its_file_changes() {
        use gg_api::asset::Meta;

        use crate::{Key, UiEvent};

        let logged = crate::testing::logged();
        let scratch = crate::dirs::tests::Scratch::new("ui-by-id");
        let root = scratch.0.join("game");
        std::fs::create_dir_all(root.join("ui")).expect("a directory");
        // Dated an hour back, so that it loads as it is.
        let written_long_ago = |name: &str, bytes: &[u8]| {
            let path = root.join(name);
            std::fs::write(&path, bytes).expect("written");
            let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
            let file = std::fs::File::options().write(true).open(path);
            (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
        };
        // Focused on `play` as it's added, a logo, a button of each name and a score.
        let menu = |texture: &str, buttons: &[&str]| {
            let buttons: String = (buttons.iter())
                .map(|name| format!("    Button(name: \"{name}\"),\n"))
                .collect();
            format!(
                "(focus: \"play\", nodes: [Column(children: [\n    Image(name: \"logo\", \
                 texture: \"{texture}\"),\n{buttons}    Text(name: \"score\", text: \"0\"),\n])])\n"
            )
        };
        let logo = "ui/logo.png";
        written_long_ago("ui/menu.ui", menu(logo, &["options", "play"]).as_bytes());
        let id = "6b7c8d9e0fa1b2c3";
        let meta = Meta {
            id: id.parse().expect("an ID"),
        };
        written_long_ago("ui/menu.ui.meta", meta.text().as_bytes());
        let (color, depth) = (png::ColorType::Rgba, png::BitDepth::Eight);
        let png = crate::image::tests::png(color, depth, 2, &[255; 8], None);
        written_long_ago(logo, &png);

        let mut cx = Context::seeded(0).root(root.clone());
        cx.hot_reload = Some(HotReload::start().expect("the thread"));
        let ui = cx.add_ui("menu", id).expect("added by its ID");
        assert!(cx.assets.texture(logo).is_some(), "what it names");
        let listed = cx.ui_list();
        let (asset, path) = (&*listed.uis[0].asset, &*listed.uis[0].path);
        assert_eq!((asset, path), (id, "ui/menu.ui"));
        assert_eq!(
            listed.uis[0].nodes[1].rect[2..],
            [2.0, 1.0],
            "its texture's size"
        );
        ui.set_text(&mut cx, "score", "120").expect("a text");
        let refused = |set: Result<()>| set.expect_err("refused").code();
        assert_eq!(refused(ui.set_text(&mut cx, "scores", "1")), "ui.no_node");
        assert_eq!(refused(ui.set_text(&mut cx, "logo", "1")), "ui.wrong_kind");
        assert_eq!(ui.focused(&mut cx).as_deref(), Some("play"), "the file's");
        let key = |key| Event::Key {
            key,
            pressed: true,
            repeat: false,
        };
        let up = ui.event(&mut cx, &key(Key::ArrowUp));
        assert_eq!(up, Some(UiEvent::Focused("options".into())));
        let again = cx.add_ui("again", root.join("ui/menu.ui")).expect("added");
        assert_eq!(cx.uis.files(), 1, "the file read once");
        assert_eq!(again.focused(&mut cx).as_deref(), Some("play"), "its own");

        // Frame starts, until `done`.
        let until = |cx: &mut Context, done: &dyn Fn(&mut Context) -> bool, what: &str| {
            let until = std::time::Instant::now() + Duration::from_secs(30);
            while !done(cx) {
                assert!(std::time::Instant::now() < until, "{what}");
                std::thread::sleep(Duration::from_millis(5));
                cx.take_reloaded(false);
            }
        };
        // Whether UI `at` has a node named `name`.
        let named = |cx: &mut Context, at: usize, name: &str| {
            let listed = cx.ui_list();
            let nodes = &listed.uis[at].nodes;
            nodes.iter().any(|node| node.name.as_deref() == Some(name))
        };
        let path = root.join("ui/menu.ui");
        std::fs::write(&path, menu(logo, &["play", "options", "settings"])).expect("an edit");
        let took = |cx: &mut Context| named(cx, 0, "settings");
        until(&mut cx, &took, "the UI takes its nodes");
        assert!(
            named(&mut cx, 1, "settings"),
            "the second UI takes them too"
        );
        // `options`, though the file focuses `play` and `options` has moved.
        assert_eq!(
            ui.focused(&mut cx).as_deref(),
            Some("options"),
            "by its name"
        );
        assert_eq!(again.focused(&mut cx).as_deref(), Some("play"));
        let listed = cx.ui_list();
        let text = |at: usize, name: &str| {
            let nodes = listed.uis[at].nodes.iter();
            let found = nodes
                .clone()
                .find(|node| node.name.as_deref() == Some(name));
            found.and_then(|node| node.text.clone())
        };
        assert_eq!(text(0, "score").as_deref(), Some("120"), "what code set");
        assert_eq!(text(1, "score").as_deref(), Some("0"));

        let _ = logged.codes.try_iter().count();
        let missing = menu("ui/none.png", &["play", "quit"]);
        std::fs::write(&path, missing).expect("an edit");
        let warned = |_: &mut Context| {
            let mut codes = logged.codes.try_iter();
            codes.any(|code| code == "asset.no_texture")
        };
        until(&mut cx, &warned, "an edit naming what doesn't load warns");
        assert!(
            named(&mut cx, 0, "settings") && !named(&mut cx, 0, "quit"),
            "as it was"
        );

        // Renamed with its ID file, as `git mv` renames both.
        let renamed = root.join("ui/main.ui");
        std::fs::rename(&path, &renamed).expect("renamed");
        std::fs::rename(Meta::path(&path), Meta::path(&renamed)).expect("renamed");
        cx.scan_assets(false);
        let listed = cx.ui_list();
        let paths: Vec<&str> = listed.uis.iter().map(|ui| &*ui.path).collect();
        assert_eq!(paths, ["ui/main.ui"; 2]);
        cx.add_ui("third", &renamed).expect("added by its new path");
        cx.add_ui("fourth", id).expect("added by its ID");
        assert_eq!(cx.uis.files(), 1, "the file read once still");
        std::fs::write(&renamed, menu(logo, &["play", "quit"])).expect("an edit");
        let quit = |cx: &mut Context| named(cx, 2, "quit") && named(cx, 0, "quit");
        until(
            &mut cx,
            &quit,
            "the file renamed reads again as it's edited",
        );
        // A new file where it was, without an ID file, is a file of its own.
        std::fs::write(&path, menu(logo, &["play", "new"])).expect("a new file");
        cx.add_ui("fifth", &path).expect("added");
        assert_eq!(cx.uis.files(), 2, "a file of its own");
        assert!(named(&mut cx, 4, "new") && !named(&mut cx, 0, "new"));
    }

    /// A UI lays out again as a font it writes in bakes again, its text measured by what it baked,
    /// and as a texture an image shows reads again at another size, the image fit to that.
    #[cfg(feature = "dev")]
    #[test]
    fn a_ui_lays_out_again_as_its_font_or_texture_reloads() {
        let scratch = crate::dirs::tests::Scratch::new("ui-font-texture");
        let root = scratch.0.join("game");
        std::fs::create_dir_all(root.join("ui")).expect("a directory");
        let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/Inter");
        let font_path = root.join("ui/a.ttf");
        std::fs::copy(fonts.join("Inter-Regular.ttf"), &font_path).expect("a font");
        let (color, depth) = (png::ColorType::Rgba, png::BitDepth::Eight);
        let image_of = |width| {
            crate::image::tests::png(color, depth, width, &vec![255; 4 * width as usize], None)
        };
        let logo = root.join("ui/logo.png");
        std::fs::write(&logo, image_of(2)).expect("a texture");
        let menu = "(nodes: [Column(children: [Image(name: \"logo\", texture: \"ui/logo.png\"), \
                    Text(name: \"label\", text: \"A WIDE LABEL\", font: \"ui/a.ttf\")])])";
        std::fs::write(root.join("ui/menu.ui"), menu).expect("a UI");
        let mut cx = Context::seeded(0).root(root.clone());
        cx.add_ui("menu", root.join("ui/menu.ui")).expect("added");
        let size = |cx: &mut Context, name: &str| {
            let listed = cx.ui_list();
            let found = listed.uis[0]
                .nodes
                .iter()
                .find(|node| node.name.as_deref() == Some(name));
            let rect = found.expect("a node").rect;
            [rect[2], rect[3]]
        };
        assert_eq!(size(&mut cx, "logo"), [2.0, 1.0]);
        let regular = size(&mut cx, "label");

        std::fs::write(&logo, image_of(4)).expect("an edit");
        let texture = cx.assets.texture("ui/logo.png").expect("loaded");
        let files = package::Files::default();
        let image = crate::image::load_image("logo", &logo, &files).expect("an image");
        let image = crate::image::decode_texture("logo", &logo, image).expect("decoded");
        cx.reload_texture(texture, &logo, image);
        assert_eq!(size(&mut cx, "logo"), [4.0, 1.0], "fit to its new size");

        let font = cx.assets.font("ui/a.ttf").expect("loaded");
        let bold = font::bake("a", &fonts.join("Inter-Bold.ttf")).expect("baked");
        cx.reload_font(font, &font_path, bold);
        let bolder = size(&mut cx, "label");
        assert!(
            bolder[0] > regular[0],
            "bold is wider: {bolder:?} {regular:?}"
        );
    }

    /// A prefab's change that the world as edited can't take, as it hasn't registered a type the
    /// text names, changes neither the world playing nor the prefab: both worlds take it, or
    /// neither does.
    #[cfg(feature = "dev")]
    #[test]
    fn a_prefab_change_one_world_refuses_changes_neither() {
        #[derive(Clone, Debug, PartialEq, crate::Component)]
        struct Lit {
            bright: f32,
        }

        let mut cx = Context::seeded(0);
        let text = "(\n    entities: {\n        \"0000000000000001\": (\n            Name: \
                    (\"lamp\"),\n        ),\n    },\n)\n";
        let prefab = cx.add_assets(|assets| {
            assets.add_prefab(
                "lamp.prefab".into(),
                "lamp.prefab".into(),
                Some(text.into()),
            )
        });
        let root = prefab.spawn(cx.world_mut(), Transform::IDENTITY);
        let root = root.expect("spawned");
        // Held aside as a copy plays, which registers a type the world as edited hasn't.
        cx.edited = Some(Edited {
            world: cx.world.clone(),
            ticks: 0,
            scenes: Vec::new(),
            cached: Vec::new(),
            open: scene_files::Open::default(),
            authors: Authors::default(),
        });
        cx.world_mut().register::<Lit>().expect("describes itself");
        let lit = text.replace(
            "Name: (\"lamp\"),",
            "Lit: (\n                bright: 2.0,\n            ),\n            Name: (\"lit\"),",
        );
        cx.reload_prefab(prefab, Path::new("lamp.prefab"), lit, false);
        let name = cx.world().get::<Name>(root).cloned();
        assert_eq!(name, Some(Name("lamp".to_owned())), "the copy as it was");
        assert_eq!(cx.world().get::<Lit>(root), None);
        let kept = cx.assets.prefab_text(prefab).map(|text| &**text);
        assert_eq!(kept, Some(text), "the prefab as it was");
    }

    /// A model that a run following another can't load stands in, as a model of no nodes, named as
    /// named, so that a scene naming it loads, its root spawning nothing; and its image, named,
    /// stands in as a texture.
    #[test]
    fn a_model_that_cant_be_loaded_stands_in_for_a_following_run() {
        let logged = crate::testing::logged();
        let mut cx = Context::seeded(0);
        cx.follows = true;
        let (model, image) = ("assets/none.glb", "assets/none.glb#0");
        cx.load_named([Asset::Model(model), Asset::Texture(image)], true);
        let stand_in = cx.assets.model(model).expect("a stand-in");
        assert_eq!(cx.assets.nodes(stand_in), Some(&Nodes::default()));
        assert!(cx.assets.texture(image).is_some(), "its image's stand-in");
        let stood_in = logged
            .codes
            .try_iter()
            .filter(|code| code == "asset.stand_in");
        assert_eq!(stood_in.count(), 2);
        let text = format!(
            "(\n    entities: {{\n        \"0000000000000100\": (\n            ModelRoot: (\"{model}\"),\n        ),\n    }},\n)\n"
        );
        let roots = cx.world_mut().load_scene(&text).expect("loaded");
        assert_eq!(cx.world().children(roots[0]).expect("alive").count(), 0);
    }

    /// A scene that names a text's font by its ID loads the font, and saves it by that ID, a text
    /// that names none saving none; and edited, the font bakes again, under its handle, its text
    /// laid out by what its file holds now.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_names_a_texts_font_by_its_id_which_bakes_again_as_it_changes() {
        use gg_api::asset::Meta;

        let scratch = crate::dirs::tests::Scratch::new("font-by-id");
        let root = scratch.0.join("game");
        for dir in ["fonts", "scenes"] {
            std::fs::create_dir_all(root.join(dir)).expect("a directory");
        }
        let path = root.join("fonts/title.ttf");
        std::fs::write(&path, font::INTER).expect("a font");
        // Dated an hour back, so that it loads as it is.
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        let file = std::fs::File::options().write(true).open(&path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
        let id = "4d5e6f708192a3b4";
        let meta = Meta {
            id: id.parse().expect("an ID"),
        };
        std::fs::write(root.join("fonts/title.ttf.meta"), meta.text()).expect("its ID file");
        let scene = root.join("scenes/s.ron");
        let text = format!(
            "(\n    entities: {{\n        \"0000000000000100\": (\n            Text: (\n\
             \x20               text: \"W\",\n                font: Some(\"{id}\"),\n\
             \x20           ),\n            Transform: (),\n        ),\n\
             \x20       \"0000000000000200\": (\n            Text: (),\n\
             \x20           Transform: (),\n        ),\n    }},\n)\n"
        );
        std::fs::write(&scene, text).expect("written");

        let mut cx = Context::seeded(0).root(root.clone());
        cx.hot_reload = Some(HotReload::start().expect("the thread"));
        let roots = cx.load_scene_file(&scene).expect("loaded");
        let title = cx.assets.font(id).expect("loaded by its ID");
        assert_eq!(cx.assets.font_path(title), "fonts/title.ttf");
        let saved = cx.world().save_scene(&roots).expect("saved");
        assert!(saved.contains(&format!("font: Some(\"{id}\")")), "{saved}");
        assert!(saved.contains("font: None"), "{saved}");

        // How wide its W is.
        let width = |cx: &mut Context| {
            let style = TextStyle {
                font: Some(title),
                size: 1.0,
                color: Vec4::ONE,
                align: [font::Align::Start; 2],
            };
            let mut glyphs = cx.fonts.lay_out(Mat4::IDENTITY, "W", &style, 0);
            glyphs.next().expect("a W").0.x_axis.x
        };
        let regular = width(&mut cx);
        let bold = include_bytes!("../assets/fonts/Inter/Inter-Bold.ttf");
        std::fs::write(&path, bold).expect("an edit");
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while width(&mut cx) == regular {
            assert!(std::time::Instant::now() < until, "the font bakes again");
            std::thread::sleep(Duration::from_millis(5));
            cx.take_reloaded(false);
        }
        assert!(width(&mut cx) > regular, "bold is wider");
        assert_eq!(cx.assets.font(id), Some(title), "under its handle");
    }

    /// Text that names by their IDs a font, a texture and a model a layer added already loads none
    /// of them again, noting nothing, in a run that names them by their IDs and in one that names
    /// them by their paths, as a lockstep joiner does until its host's names come: so that both
    /// note what their layers added alike, by which what's opaque in the host's world comes back
    /// in the joiner's. The run that names them by their paths names them by the IDs too from then
    /// on.
    #[cfg(feature = "dev")]
    #[test]
    fn text_naming_by_id_what_a_layer_added_notes_nothing_however_the_run_names_it() {
        use gg_api::asset::Meta;

        let scratch = crate::dirs::tests::Scratch::new("named-loaded");
        let root = scratch.0.join("game");
        for dir in ["fonts", "textures", "models"] {
            std::fs::create_dir_all(root.join(dir)).expect("a directory");
        }
        let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let model = gltf::tests::model();
        let files: [(&str, &[u8], &str); 3] = [
            ("fonts/title.ttf", font::INTER, "4d5e6f708192a3b4"),
            ("textures/tile.png", &png, "1a2b3c4d5e6f7081"),
            ("models/m.glb", &model, "8192a3b4c5d6e7f8"),
        ];
        for (path, bytes, id) in files {
            std::fs::write(root.join(path), bytes).expect("an asset");
            let meta = Meta {
                id: id.parse().expect("an ID"),
            };
            std::fs::write(root.join(format!("{path}.meta")), meta.text()).expect("its ID file");
        }
        let [
            (font_path, _, font_id),
            (texture_path, _, texture_id),
            (model_path, _, model_id),
        ] = files;
        let mut notes = Vec::new();
        for by_paths in [false, true] {
            let mut cx = Context::seeded(0).root(root.clone());
            if by_paths {
                cx.name_as([]);
            }
            let font = cx.add_font("title", root.join(font_path)).expect("a font");
            let texture = cx.add_texture("tile", root.join(texture_path));
            let texture = texture.expect("a texture");
            let model = cx.add_model("m", root.join(model_path)).expect("a model");
            let added = cx.assets.added().to_vec();
            let named = [
                Asset::Font(font_id),
                Asset::Texture(texture_id),
                Asset::Model(model_id),
            ];
            cx.load_named(named, true);
            assert_eq!(cx.assets.added(), added, "noted again");
            assert_eq!(cx.assets.font(font_id), Some(font));
            assert_eq!(cx.assets.texture(texture_id), Some(texture));
            assert_eq!(cx.assets.model(model_id), Some(model));
            notes.push(added);
        }
        assert_eq!(notes[0], notes[1]);
    }

    /// A font that a run following another can't load stands in, named as named, so that a scene
    /// naming it loads, its text drawing in the engine's own font.
    #[cfg(feature = "dev")]
    #[test]
    fn a_font_that_cant_be_loaded_stands_in_for_a_following_run() {
        let logged = crate::testing::logged();
        let mut cx = Context::seeded(0);
        cx.follows = true;
        let name = "fonts/none.ttf";
        cx.load_named([Asset::Font(name)], true);
        let stand_in = cx.assets.font(name).expect("a stand-in");
        let stood_in = logged
            .codes
            .try_iter()
            .filter(|code| code == "asset.stand_in");
        assert_eq!(stood_in.count(), 1);
        let text = format!(
            "(\n    entities: {{\n        \"0000000000000100\": (\n            Text: (\n\
             \x20               font: Some(\"{name}\"),\n            ),\n        ),\n    }},\n)\n"
        );
        cx.world_mut().load_scene(&text).expect("loaded");
        let style = TextStyle {
            font: Some(stand_in),
            ..font::Text::default().style()
        };
        let inter = cx.fonts.inter_field();
        let mut glyphs = cx.fonts.lay_out(Mat4::IDENTITY, "Hi", &style, 0);
        assert!(glyphs.all(|(_, glyph, _)| glyph.field == inter));
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

    /// Each texture, model, scene file, prefab, font, UI, sound and music loaded logs how long it
    /// took, a texture's image as it decodes too, a model's images among them, and a font's bake;
    /// but a file added again, which loads nothing, logs nothing, and nor does a scene that fails
    /// to load, whose error says why.
    #[test]
    fn loads_log_how_long_they_took() {
        let logged = crate::testing::logged();
        let scratch = crate::dirs::tests::Scratch::new("loads-logged");
        let root = scratch.0.clone();
        for dir in ["assets", "scenes"] {
            std::fs::create_dir_all(root.join(dir)).expect("a directory");
        }
        let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let write = |name: &str, bytes: &[u8]| std::fs::write(root.join(name), bytes);
        write("assets/stone.png", &png).expect("written");
        write("assets/plain.png", &png).expect("written");
        write("assets/m.glb", &gltf::tests::model()).expect("written");
        let inter = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/Inter");
        let font = std::fs::read(inter.join("Inter-Regular.ttf")).expect("the engine's font");
        write("assets/inter.ttf", &font).expect("written");
        write("assets/menu.ui", b"(nodes: [Button(name: \"play\")])").expect("written");
        let wav = audio::tests::wav(&[0; 480]);
        write("assets/chime.wav", &wav).expect("written");
        write("assets/theme.wav", &wav).expect("written");
        // A scene of a sprite of the texture named, whose ID is `id`, written to `file`.
        let scene = |file: &str, id: u64, name: &str| -> PathBuf {
            let sprite = format!("(Sprite: (texture: Some(\"{name}\")))");
            let text = format!("(entities: {{\"{id:016x}\": {sprite}}})");
            let path = root.join(file);
            std::fs::write(&path, text).expect("written");
            path
        };
        let mut cx = Context::seeded(0).root(root.clone());
        let _ = logged.codes.try_iter().count();
        cx.add_texture("stone", root.join("assets/stone.png"))
            .expect("a texture");
        cx.add_model("m", root.join("assets/m.glb"))
            .expect("a model");
        cx.add_texture("again", root.join("assets/stone.png"))
            .expect("it again");
        cx.add_model("again", root.join("assets/m.glb"))
            .expect("it again");
        cx.load_scene_file(scene("scenes/s.ron", 0x1000, "assets/plain.png"))
            .expect("loaded");
        let error = cx.load_scene_file(scene("scenes/s.ron", 0x2000, "assets/none.png"));
        assert_eq!(error.expect_err("no file").code(), "asset.no_texture");
        // Of a texture loaded already, which it loads nothing of.
        let prefab = scene("assets/p.prefab", 0x3000, "assets/plain.png");
        cx.add_prefab("p", &prefab).expect("a prefab");
        cx.add_prefab("again", &prefab).expect("it again");
        cx.add_font("inter", root.join("assets/inter.ttf"))
            .expect("a font");
        cx.add_ui("menu", root.join("assets/menu.ui"))
            .expect("a UI");
        cx.add_sound("chime", root.join("assets/chime.wav"))
            .expect("a sound");
        cx.add_music("theme", root.join("assets/theme.wav"))
            .expect("music");
        let codes: Vec<String> = (logged.codes.try_iter())
            .filter(|code| code.starts_with("asset.") || code.starts_with("scene."))
            .collect();
        // A test's context decodes each image as it's added, so each logs before its add does.
        let decoded = "asset.texture_decoded";
        assert_eq!(
            codes,
            [
                decoded,
                "asset.texture_loaded",
                decoded,
                decoded,
                "asset.model_loaded",
                decoded,
                "asset.texture_loaded",
                "scene.loaded",
                "asset.prefab_loaded",
                "asset.font_baked",
                "asset.font_loaded",
                "asset.ui_loaded",
                "asset.sound_loaded",
                "asset.music_loaded",
            ]
        );
    }

    /// Frames handed off, as a run's do, at a view of 64 pixels square, until the context's loader
    /// decodes nothing, which each frame takes what it decoded of.
    fn decoded(cx: &mut Context) {
        let view = Viewport {
            origin: [0, 0],
            size: [64, 64],
        };
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while cx.streaming().decoding > 0 {
            assert!(std::time::Instant::now() < until, "the images decode");
            std::thread::sleep(Duration::from_millis(1));
            cx.extract(&mut render_thread::Extracted::default(), view);
        }
    }

    /// A texture whose header reads but whose pixels don't decode, as a PNG cut short in its rows,
    /// loads, its handle and name as any texture's, and warns as its pixels are found not to
    /// decode: off the main thread in a run's context, and at once in a test's or a replay's, so
    /// that both add it alike, as they do a model whose image is such a file.
    #[test]
    fn a_texture_that_reads_but_doesnt_decode_loads_and_warns() {
        let logged = crate::testing::logged();
        let scratch = crate::dirs::tests::Scratch::new("cut-texture");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let mut png = image::tests::png(
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            4,
            &[9; 16],
            None,
        );
        png.truncate(png.len() - 20);
        let path = scratch.0.join("cut.png");
        std::fs::write(&path, png).expect("a PNG, cut");
        #[cfg(feature = "dev")]
        let model = {
            let gltf = gltf::tests::triangle_gltf(|gltf| {
                gltf["images"][0] = serde_json::json!({"uri": "cut.png"});
            });
            let model = scratch.0.join("m.gltf");
            std::fs::write(&model, gltf).expect("a model");
            model
        };
        for threaded in [false, true] {
            let mut cx = Context::seeded(0).root(scratch.0.clone());
            if threaded {
                cx.loader = Loader::threaded();
            }
            let _ = logged.codes.try_iter().count();
            let cut = cx.add_texture("cut", &path);
            let cut = cut.expect("loaded, as its header reads");
            assert_eq!(cx.assets.path(cut), Some("cut.png"));
            #[cfg(feature = "dev")]
            (cx.add_model("m", &model)).expect("loaded, as its image's header reads");
            // The texture's image, and, with `dev`, the model's.
            let warned = if cfg!(feature = "dev") { 2 } else { 1 };
            assert_eq!(cx.streaming().decoding, if threaded { warned } else { 0 });
            decoded(&mut cx);
            let codes = logged.codes.try_iter();
            let bad = codes.filter(|code| code == "engine.bad_texture");
            assert_eq!(bad.count(), warned as usize, "threaded: {threaded}");
        }
    }

    /// A texture removed keeps its name, which finds it still, so that the world's hash reads
    /// nothing of the removal; but where text names it, it loads anew, under a handle of its own,
    /// which its names find from then on, as a layer's add of its file gives. Its removal is noted
    /// once, by its index and its path; and the engine's own textures aren't removed.
    #[test]
    fn a_removed_texture_keeps_its_name_and_loads_anew_where_named() {
        let scratch = crate::dirs::tests::Scratch::new("removed-texture");
        let root = scratch.0.clone();
        std::fs::create_dir_all(root.join("assets")).expect("a directory");
        let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let path = root.join("assets/stone.png");
        std::fs::write(&path, png).expect("a PNG");
        let named = "assets/stone.png";
        let mut cx = Context::seeded(0).root(root);
        let stone = cx.add_texture("stone", &path).expect("a texture");
        let sprite = Sprite {
            texture: Some(stone),
            ..Sprite::default()
        };
        cx.world_mut().spawn((Transform::IDENTITY, sprite));
        let hash = cx.world().hash();
        cx.remove_texture(stone);
        cx.remove_texture(stone);
        assert_eq!(cx.world().hash(), hash, "the world reads nothing of it");
        assert_eq!(
            cx.assets.texture(named),
            Some(stone),
            "its name finds it still"
        );
        assert!(
            !cx.assets.has(&Asset::Texture(named)),
            "but nothing loaded has it"
        );
        let note = format!("removed texture {} {named}", stone.index());
        let notes = cx.assets.added().iter().filter(|added| ***added == *note);
        assert_eq!(notes.count(), 1, "noted once");

        cx.load_named([Asset::Texture(named)], false);
        let anew = cx.assets.texture(named).expect("loaded anew");
        assert!(anew.index() > stone.index(), "under a handle of its own");
        assert_eq!(cx.assets.name(anew), Some(named));
        let added = cx.add_texture("again", &path).expect("its file again");
        assert_eq!(added, anew, "loaded once, anew");
        assert_eq!(cx.world().hash(), hash);
        #[cfg(feature = "dev")]
        {
            assert!(cx.has_added(named));
            cx.remove_texture(anew);
            assert!(!cx.has_added(named));
        }
        let white = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cx.remove_texture(Texture::WHITE);
        }));
        assert!(white.is_err(), "the engine's white texture");
    }

    /// The draws under `entity`, a model's root, and under each of its nodes, in order.
    fn drawn(world: &World, entity: Entity) -> Vec<Option<MeshDraw>> {
        let mut drawn = vec![world.get::<MeshDraw>(entity).copied()];
        for child in world.children(entity).expect("alive") {
            drawn.extend(self::drawn(world, child));
        }
        drawn
    }

    /// A model removed takes its meshes and images with it, noted once, and keeps its nodes, so
    /// that a root spawned of it after spawns what one before did; but its file loads anew as
    /// another model; and its own meshes and images aren't removed alone.
    #[test]
    fn a_removed_model_takes_its_meshes_and_images_and_spawns_alike() {
        let scratch = crate::dirs::tests::Scratch::new("removed-model");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let path = scratch.0.join("m.glb");
        std::fs::write(&path, gltf::tests::model()).expect("a model");
        let mut cx = Context::seeded(0).root(scratch.0.clone());
        let model = cx.add_model("m", &path).expect("a model");
        let made = cx.loaded.made[model.index()].clone();
        assert!(!made.meshes.is_empty() && !made.images.is_empty());
        let before = model.spawn(cx.world_mut(), Transform::IDENTITY);
        cx.remove_model(model);
        cx.remove_model(model);
        let after = model.spawn(cx.world_mut(), Transform::IDENTITY);
        let world = cx.world();
        assert_eq!(drawn(world, after), drawn(world, before), "spawned alike");
        assert!(
            made.meshes.iter().all(|&mesh| !cx.meshes.remove(mesh)),
            "its meshes gone"
        );
        assert!(
            made.images
                .iter()
                .all(|&(image, _)| cx.assets.removed(image))
        );
        assert_eq!(
            cx.assets.model("m.glb"),
            Some(model),
            "its name finds it still"
        );
        assert!(!cx.assets.has(&Asset::Model("m.glb")));
        let note = format!("removed model {} m.glb", model.index());
        let notes = cx.assets.added().iter().filter(|added| ***added == *note);
        assert_eq!(notes.count(), 1, "noted once");

        let anew = cx.add_model("m", &path).expect("its file again");
        assert_ne!(anew, model);
        assert_eq!(cx.assets.model("m.glb"), Some(anew));
        let made = cx.loaded.made[anew.index()].clone();
        let (mesh, image) = (made.meshes[0], made.images[0].0);
        let alone = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cx.remove_mesh(mesh);
        }));
        assert!(alone.is_err(), "a model's mesh alone");
        let alone = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cx.remove_texture(image);
        }));
        assert!(alone.is_err(), "a model's image alone");
    }

    /// A texture removed is watched no more: an edit of its file reads nothing of it, where one of
    /// another texture's reads that.
    #[cfg(feature = "dev")]
    #[test]
    fn a_removed_texture_is_watched_no_more() {
        let scratch = crate::dirs::tests::Scratch::new("removed-unwatched");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let png = |width: u32| {
            let pixels = vec![9; width as usize * 4];
            image::tests::png(
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                width,
                &pixels,
                None,
            )
        };
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        for name in ["gone.png", "kept.png"] {
            let path = scratch.0.join(name);
            std::fs::write(&path, png(1)).expect("a PNG");
            // Dated an hour back, so that each loads as it is.
            let file = std::fs::File::options().write(true).open(path);
            (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
        }
        let mut cx = Context::seeded(0).root(scratch.0.clone());
        cx.hot_reload = Some(HotReload::start().expect("the thread"));
        let gone = cx.add_texture("gone", scratch.0.join("gone.png"));
        let gone = gone.expect("a texture");
        let kept = cx.add_texture("kept", scratch.0.join("kept.png"));
        let kept = kept.expect("a texture");
        cx.remove_texture(gone);
        for name in ["gone.png", "kept.png"] {
            std::fs::write(scratch.0.join(name), png(2)).expect("an edit");
        }
        let mut read = Vec::new();
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while !read.contains(&kept) {
            assert!(
                std::time::Instant::now() < until,
                "the kept texture reads again"
            );
            std::thread::sleep(Duration::from_millis(5));
            let hot_reload = cx.hot_reload.as_ref().expect("started");
            while let Some(reloaded) = hot_reload.take() {
                if let Reloaded::Texture { texture, .. } = reloaded {
                    read.push(texture);
                }
            }
        }
        assert!(!read.contains(&gone), "the removed one read again");
    }

    /// A sound's file added again, by another form of its path, gives the sound it gave first,
    /// without decoding it again; a file that isn't a sound, or isn't there, is refused, naming the
    /// sound and its file.
    #[test]
    fn a_sound_decodes_once_however_its_path_is_written() {
        let scratch = crate::dirs::tests::Scratch::new("sound-once");
        let root = scratch.0.clone();
        std::fs::create_dir_all(root.join("sounds")).expect("a directory");
        std::fs::write(root.join("sounds/chime.wav"), audio::tests::wav(&[0; 480])).expect("a WAV");
        std::fs::write(root.join("sounds/noise.wav"), b"RIFF, but no more").expect("a file");
        let mut cx = Context::seeded(0).root(root.clone());
        let chime = cx.add_sound("chime", root.join("sounds/chime.wav"));
        let chime = chime.expect("a sound");
        let dotted = root.join("./sounds/../sounds/chime.wav");
        assert_eq!(cx.add_sound("again", &dotted).expect("it again"), chime);
        assert_eq!(cx.audio.frames(chime), 480);
        for (name, path) in [("noise", "sounds/noise.wav"), ("none", "sounds/none.wav")] {
            let path = root.join(path);
            let error = cx.add_sound(name, &path).expect_err("no sound");
            assert_eq!(error.code(), "engine.bad_sound");
            let message = error.to_string();
            let named = message.contains(&format!("{name:?}"));
            assert!(
                named && message.contains(&path.display().to_string()),
                "{message}"
            );
        }
    }

    /// Queues a sound at each tick, at its entity, as a game's tick does.
    struct Chime {
        sound: Sound,
        on: Entity,
    }

    impl Layer for Chime {
        fn tick(&mut self, world: &mut World, _: Tick) {
            world.play_sound(self.sound, Place::On(self.on));
        }
    }

    /// The sounds a tick queues play once it has run, every layer's, so that the world holds none
    /// between ticks, in a run that plays sound or not.
    #[test]
    fn the_sounds_a_tick_queues_play_once_it_has_run() {
        let scratch = crate::dirs::tests::Scratch::new("sound-tick");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let path = scratch.0.join("chime.wav");
        std::fs::write(&path, audio::tests::wav(&[0; 480])).expect("a WAV");
        let mut cx = Context::seeded(0);
        let sound = cx.add_sound("chime", &path).expect("a sound");
        let on = cx.world_mut().spawn(Transform::IDENTITY);
        let mut stack = Stack::default();
        stack.push_layer(Chime { sound, on });
        stack.push_overlay(Chime { sound, on });
        for number in 0..2 {
            stack.tick(&mut cx, Tick { number });
            assert_eq!(cx.world().sounds_queued().count(), 0);
        }
        cx.world_mut().play_sound(sound, Place::Nowhere);
        let queued: Vec<_> = cx.world().sounds_queued().collect();
        assert_eq!(queued, [(sound, Place::Nowhere)]);
    }

    /// A sound's file edited decodes again, under its handle, its plays taking what the file holds
    /// now.
    #[cfg(feature = "dev")]
    #[test]
    fn a_sound_decodes_again_as_its_file_changes() {
        let scratch = crate::dirs::tests::Scratch::new("sound-reload");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let path = scratch.0.join("chime.wav");
        std::fs::write(&path, audio::tests::wav(&[0; 480])).expect("a WAV");
        // Dated an hour back, so that it loads as it is.
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        let file = std::fs::File::options().write(true).open(&path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
        let mut cx = Context::seeded(0);
        cx.hot_reload = Some(HotReload::start().expect("the thread"));
        let sound = cx.add_sound("chime", &path).expect("a sound");
        std::fs::write(&path, audio::tests::wav(&[0; 960])).expect("an edit");
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while cx.audio.frames(sound) == 480 {
            assert!(std::time::Instant::now() < until, "the sound decodes again");
            std::thread::sleep(Duration::from_millis(5));
            cx.take_reloaded(false);
        }
        assert_eq!(cx.audio.frames(sound), 960);
    }

    /// Music's file added again, by another form of its path, gives the music it gave first,
    /// without reading it again; a file that isn't music, or isn't there, is refused, naming the
    /// music and its file.
    #[test]
    fn music_reads_once_however_its_path_is_written() {
        let scratch = crate::dirs::tests::Scratch::new("music-once");
        let root = scratch.0.clone();
        std::fs::create_dir_all(root.join("music")).expect("a directory");
        std::fs::write(root.join("music/theme.wav"), audio::tests::wav(&[0; 480])).expect("a WAV");
        std::fs::write(root.join("music/noise.ogg"), b"OggS, but no more").expect("a file");
        let mut cx = Context::seeded(0).root(root.clone());
        let theme = cx.add_music("theme", root.join("music/theme.wav"));
        let theme = theme.expect("music");
        let dotted = root.join("./music/../music/theme.wav");
        assert_eq!(cx.add_music("again", &dotted).expect("it again"), theme);
        assert_eq!(cx.audio.music_frames(theme), 480);
        for (name, path) in [("noise", "music/noise.ogg"), ("none", "music/none.ogg")] {
            let path = root.join(path);
            let error = cx.add_music(name, &path).expect_err("no music");
            assert_eq!(error.code(), "engine.bad_music");
            let message = error.to_string();
            let named = message.contains(&format!("{name:?}"));
            assert!(
                named && message.contains(&path.display().to_string()),
                "{message}"
            );
        }
    }

    /// Music's file edited reads again, under its handle, as it plays, its plays taking what the
    /// file holds now.
    #[cfg(feature = "dev")]
    #[test]
    fn music_reads_again_as_its_file_changes() {
        let scratch = crate::dirs::tests::Scratch::new("music-reload");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let path = scratch.0.join("theme.wav");
        std::fs::write(&path, audio::tests::wav(&[0; 480])).expect("a WAV");
        // Dated an hour back, so that it loads as it is.
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        let file = std::fs::File::options().write(true).open(&path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
        let mut cx = Context::seeded(0);
        cx.hot_reload = Some(HotReload::start().expect("the thread"));
        let theme = cx.add_music("theme", &path).expect("music");
        cx.play_music(Some(theme), Duration::ZERO);
        std::fs::write(&path, audio::tests::wav(&[0; 960])).expect("an edit");
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while cx.audio.music_frames(theme) == 480 {
            assert!(std::time::Instant::now() < until, "the music reads again");
            std::thread::sleep(Duration::from_millis(5));
            cx.take_reloaded(false);
        }
        assert_eq!(cx.audio.music_frames(theme), 960);
        assert_eq!(cx.music_playing(), Some(theme));
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
        assert_eq!(frame.outline, Some((orange, 3.0)));
        assert_eq!(cx.outlined(), None);
        cx.extract(&mut frame, viewport);
        assert_eq!(frame.outline, None, "the next frame outlines nothing");
    }

    /// A frame hands the renderer the IDs of the outlined entity and of every entity under it,
    /// sorted: a parent's children and theirs, but not its siblings or its own parent; a model's
    /// root, which draws nothing, its nodes. An entity that's gone gives none, and the next frame
    /// reuses the IDs' memory.
    #[test]
    fn an_outline_covers_the_entity_and_every_entity_under_it() {
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        let mut frame = render_thread::Extracted::default();
        let mut cx = Context::seeded(1);
        let (model, _) = crate::model::tests::added(&mut cx, "m", crate::model::tests::imported());
        let world = cx.world_mut();
        let root = model.spawn(world, Transform::IDENTITY);
        let [above, parent, child, grandchild, sibling] =
            [(); 5].map(|()| world.spawn(Transform::IDENTITY));
        for (entity, under) in [(parent, above), (child, parent), (grandchild, child)] {
            world.set_parent(entity, Some(under)).expect("alive");
        }
        world.set_parent(sibling, Some(above)).expect("alive");
        let id = |world: &World, entity| world.id(entity).expect("alive");
        let sorted = |mut ids: Vec<u64>| {
            ids.sort_unstable();
            ids
        };
        let outline = |cx: &mut Context, frame: &mut render_thread::Extracted, id| {
            cx.outline(id, Vec4::ONE, 2.0);
            cx.extract(frame, viewport);
            frame.outline_ids.clone()
        };

        let want = [parent, child, grandchild].map(|entity| id(cx.world(), entity).get());
        let parent_id = id(cx.world(), parent);
        let got = outline(&mut cx, &mut frame, parent_id);
        assert_eq!(got, sorted(want.to_vec()));

        let world = cx.world();
        let nodes = world.query::<(Entity, &ModelNode)>();
        let nodes = nodes.filter(|&(node, _)| world.node_root(node) == root);
        let mut want: Vec<u64> = nodes.map(|(node, _)| id(world, node).get()).collect();
        assert!(want.len() > 1, "a model with nodes");
        let root_id = id(world, root);
        want.push(root_id.get());
        let got = outline(&mut cx, &mut frame, root_id);
        assert_eq!(got, sorted(want));

        // Gone: outlined, but with no IDs to match.
        let gone = id(cx.world(), grandchild);
        cx.world_mut().despawn(grandchild).expect("alive");
        assert_eq!(outline(&mut cx, &mut frame, gone), Vec::<u64>::new());
        assert!(frame.outline.is_some());
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
        fn swapped(&mut self, _: &mut Context) {
            self.calls.push("swapped");
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
        hot.swapped(&mut cx);
        hot.tick(&mut world, Tick { number: 0 });
        let queued = hot.commands().map(|commands| commands.len());
        hot.update(&mut cx, Duration::ZERO);
        let handled = hot.event(&mut cx, &Event::CursorEntered);
        hot.detach(&mut cx);
        assert_eq!((queued, handled), (Some(1), ControlFlow::Break(())));
        let calls = [
            "load", "attach", "restored", "swapped", "tick", "commands", "update", "event",
            "detach",
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
                value: Value::F64(f64::from(x)),
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
