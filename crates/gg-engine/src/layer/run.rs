//! Making a context, and the run it's in: its sessions, window, clipboard, user's state, input.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gg_core::math::DVec3;
use gg_core::{Error, Result};
use gg_renderer::{Circle, Draws, Materials, Meshes, Scene, Shaders, Sprite, Streaming, Textures};
use tracing::error;

#[cfg(doc)]
use super::Layer;
use super::{Asked, Context, Dev, OwnSessions, Window};
#[cfg(any(test, doc))]
use crate::Event;
use crate::args::{Listen, Net, Toward};
#[cfg(feature = "dev")]
use crate::assets;
use crate::assets::{Assets, Loaded};
use crate::audio::Audio;
use crate::camera::{self, Camera};
use crate::canvas;
use crate::cells::{CellRun, CellState, Preparing};
use crate::draw::Scratch;
use crate::edits::Authors;
use crate::environment::Environment;
use crate::font::{self, Fonts};
use crate::game_ui;
use crate::interpolation::Past;
use crate::light::{self, Light};
use crate::loader::Loader;
use crate::navigation::{NavAgent, NavMeshVolume};
use crate::net::{Code, Session, SessionStatus};
use crate::opaque::OpaqueMeshes;
use crate::package::Files;
use crate::perf::Perf;
use crate::physics::{
    BoxCollider3D, CapsuleCollider3D, CharacterController3D, CircleCollider2D, Joint2D, Joint3D,
    MeshCollider3D, Physics2D, Physics3D, RigidBody3D, SphereCollider3D,
};
use crate::placed::Placed;
use crate::scene_files::SceneFiles;
use crate::skins::Skins;
use crate::{
    AnimationStates, Animator, App, BoxCollider2D, Flipbook, Input, MeshDraw, ModelNode, ModelRoot,
    Name, Owner, ParticleEmitter, PrefabRoot, RigidBody2D, Transform, Tweens, World, dirs,
};

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
                cx.dev.folders.clone_from(&manifest.scenes);
                cx.dev.project_dir = manifest.path.parent().map(Path::to_owned);
            }
            cx.dev.asset_cache =
                (cx.root.as_deref()).and_then(|root| assets::cache(app.name, root));
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
            cx.dev.folders = manifest.scenes;
            cx.dev.project_dir = Some(dir.as_ref().to_owned());
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
            dev: Dev::new(),
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
            state: None,
            assets,
            loaded: Loaded::default(),
            follows: false,
            perf: Perf::default(),
            paused: false,
            exiting: false,
            failed: false,
            load_started: None,
            authors: Authors::default(),
            scenes: SceneFiles::default(),
            hosts: crate::luau::Hosts::default(),
            mods: crate::luau::RunMods::default(),
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
        }
    }

    /// Whether a SyncTest runs each tick again on a copy of the world (`--sync-test`), as
    /// [`Layer::swapped`] says: for the Inspector, whose tick then keeps its log as the tick
    /// started, for the tick's run again.
    #[cfg(feature = "dev")]
    pub(crate) fn sync_tested(&self) -> bool {
        self.dev.sync_tested
    }

    /// The player this run plays: 0 for a lockstep host (`--host`, or [`host`](Self::host)), 1 for
    /// its joiner (`--join`, or [`join`](Self::join)), the one its server gives a client
    /// (`--connect`); `None` for a server and outside a session, as once a session the game
    /// started ends. Each peer runs every player's world, and its layers queue commands for their
    /// own player alone, naming it in them, as the other peer's queue for theirs: a command whose
    /// variant or struct has a field `player`, an integer, that names another player than the one
    /// whose inputs it came in, is refused there (`net.bad_command`), by every run of the session
    /// alike.
    pub fn player(&self) -> Option<u8> {
        self.player
    }

    /// Whether the run takes part in a networked session: a lockstep session's, as its host
    /// (`--host`) or its joiner (`--join`), or a server's, as the server (`--serve`) or a client
    /// (`--connect`); not a run alone, a replay or a test's context. A game tells by it what only a
    /// run alone has, as a player the game plays itself, which a server's world, whose
    /// [`player`](Self::player) is `None` as a run alone's is, mustn't have. It changes as a
    /// session the game hosts or joins starts, and as it leaves it, from the frame after, which a
    /// layer that sets up such a player follows, taking its bot off and putting it back.
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
}
