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

    /// A context for `app`'s run, in `window` if any. Entity IDs come from the app's seed, else a
    /// random one. Textures decode off the main thread. It plays sound if `aloud`
    /// ([`add_sound`](Self::add_sound)).
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
        {
            cx.dev.embeds_game = app.embeds_game;
        }
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

    /// Whether the run reads its asset root's files from its game's package
    /// ([`App::project`](crate::App::project)), not the disk. It reloads nothing and has no asset
    /// database, scene files to open or save, or ID files.
    pub(crate) fn reads_package(&self) -> bool {
        self.files.packaged().is_some()
    }

    /// A context without a window, renderer or stack, drawing entity IDs from `seed`. For calling
    /// a layer's [`attach`](Layer::attach) and [`tick`](Layer::tick)s directly, as a test does,
    /// so the simulation is the same on every run and machine. Textures decode on the calling
    /// thread as they're added; pixels that don't decode load and warn, as in a run.
    #[doc(hidden)]
    pub fn seeded(seed: u64) -> Self {
        Self::with_world(None, World::seeded(seed))
    }

    /// Names assets by their paths relative to `root` ([`App::root`](crate::App::root)), and in a
    /// development build by the IDs in the ID files under it, which it reads and never writes. For
    /// a [`seeded`](Self::seeded) context: the world's hash reads textures' names, which must
    /// match a game's runs wherever its files are.
    #[doc(hidden)]
    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        #[cfg(feature = "dev")]
        self.scan_assets(false);
        self
    }

    /// Makes the game the project whose manifest is in `dir`
    /// ([`App::project`](crate::App::project)), naming assets as [`root`](Self::root) does and
    /// loading its start scene. For a [`seeded`](Self::seeded) context.
    ///
    /// Fails as reading the manifest does in `App::project`.
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

    /// Has the layers' [`Mods`](crate::Mods) load the mods in `dir` for game `game`, as `--mods`
    /// does. For a [`seeded`](Self::seeded) context.
    #[doc(hidden)]
    #[must_use]
    pub fn mods(mut self, game: &str, dir: impl Into<PathBuf>) -> Self {
        self.mods.game = game.to_owned();
        self.mods.dir = Some(dir.into());
        self
    }

    /// Makes the context [`networked`](Self::networked), playing `player` (`None` for a server),
    /// so a [`seeded`](Self::seeded) test calls a layer's `attach` and `restored` as a session's
    /// run does.
    #[doc(hidden)]
    #[must_use]
    pub fn in_session(mut self, player: Option<u8>) -> Self {
        self.networked = true;
        self.player = player;
        self
    }

    /// Has the sessions layers host and join go through the relay at `relay`, as `--relay` alone
    /// does, for a [`seeded`](Self::seeded) context. No frame takes the session up;
    /// [`set_session`](Self::set_session) stands for that.
    #[doc(hidden)]
    #[must_use]
    pub fn with_relay(mut self, relay: &str) -> Self {
        self.own.relay = Some(relay.to_owned());
        self.own.app = "Tested";
        self
    }

    /// Sets the session to `status`, or none, as a frame that takes up or leaves a session does;
    /// [`session`](Self::session), [`networked`](Self::networked) and [`player`](Self::player)
    /// follow. For a [`seeded`](Self::seeded) context.
    #[doc(hidden)]
    pub fn set_session(&mut self, status: Option<SessionStatus>) {
        self.networked = status.is_some();
        self.player = status.as_ref().map(SessionStatus::player);
        self.session = status;
    }

    fn with_world(window: Option<Window>, mut world: World) -> Self {
        // The engine's names first, so a game's types can't take them, before any entity has one.
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
            // For a test or replay, which draws nothing: decodes on the calling thread.
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

    /// Whether a SyncTest reruns each tick on a copy of the world (`--sync-test`,
    /// [`Layer::swapped`]). The Inspector's tick then keeps its log as the tick started, for the
    /// rerun.
    #[cfg(feature = "dev")]
    pub(crate) fn sync_tested(&self) -> bool {
        self.dev.sync_tested
    }

    /// The player this run plays: 0 for a lockstep host (`--host`, [`host`](Self::host)), 1 for
    /// its joiner (`--join`, [`join`](Self::join)), the one its server gives a client
    /// (`--connect`). `None` for a server and outside a session, also once a session the game
    /// started ends.
    ///
    /// Each peer runs every player's world, and its layers queue commands for their own player
    /// alone, naming it in them. A command whose variant or struct has an integer field `player`
    /// naming another player than the one whose inputs it came in is refused (`net.bad_command`),
    /// by every run of the session alike.
    pub fn player(&self) -> Option<u8> {
        self.player
    }

    /// Whether the run takes part in a networked session: a lockstep host (`--host`) or joiner
    /// (`--join`), or a server (`--serve`) or client (`--connect`). Not a run alone, a replay or a
    /// test's context.
    ///
    /// A game tells by it what only a run alone has, such as a player it plays itself, which a
    /// server's world must not have though its [`player`](Self::player) is `None` as a run
    /// alone's is. It changes from the frame after a hosted or joined session starts or ends; a
    /// layer that sets up such a player follows, taking its bot off and putting it back.
    pub fn networked(&self) -> bool {
        self.networked
    }

    /// Hosts a lockstep session through the run's relay, as `--host` with `--relay` alone would,
    /// from the next frame. The run becomes player 0 and plays on alone until a run joins by the
    /// code [`session`](Self::session) gives, for the game's UI to show. Its peers predict each
    /// other's inputs if the app's sessions do ([`App::predicting`]).
    ///
    /// The relay is the one `--relay` names without a session flag, else the project
    /// manifest's (a build without `dev` reads it from its package). Games' sessions use one, as
    /// runs behind routers need.
    ///
    /// Fails if the run:
    ///
    /// - takes part in a session already, or has one started (`net.busy`);
    /// - names no relay (`net.no_relay`);
    /// - is recorded (`net.recorded`): a recording holds a run alone or a session from its start;
    /// - can't read its build (`net.build`) or make the code (`net.start`).
    ///
    /// Reads the run's executable to name its build, as a session does as it starts: some
    /// milliseconds on the calling thread.
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

    /// Joins the lockstep session `code` names through the run's relay, as `--join` with `--relay`
    /// alone and `--code` would, from the next frame, as player 1. It runs no tick until the
    /// host's world replaces its own, its layers picking up as [`Layer::restored`] says;
    /// [`session`](Self::session) says how it goes meanwhile. `code` is read as a player types it:
    /// any case, hyphens and spaces skipped, `I` and `L` as `1`, `O` as `0`.
    ///
    /// Fails as [`host`](Self::host) does, and if `code` isn't a session's (`net.code`); the error
    /// omits the code, a session's key.
    pub fn join(&mut self, code: &str) -> Result<()> {
        let relay = self.startable()?;
        let code: Code = code.parse().map_err(|why: String| {
            Error::new("net.code", format!("that isn't a session's code: {why}"))
        })?;
        self.start_session(&Net::Join(Toward::Relay(relay)), code)
    }

    /// The relay for [`host`](Self::host) and [`join`](Self::join), if the run can start a session.
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

    /// Starts the session `net` says, keyed by `code`, for the stack to take up next frame.
    fn start_session(&mut self, net: &Net, code: Code) -> Result<()> {
        let own = &self.own;
        let (predicting, link) = (own.predicting, own.link);
        let (app, build, layers) = (own.app, &own.build, &own.layers);
        let session = Session::begin(net, Some(code), app, build, layers, predicting, link)?;
        self.asked = Some(Asked::Start(Box::new(session)));
        Ok(())
    }

    /// Leaves the run's session, if any, from the next frame. The run plays on alone from its
    /// world (a joiner's is its host's), no longer [`networked`](Self::networked), its
    /// [`player`](Self::player) `None`. The other run loses it, as when a connection closes. A
    /// session started this frame ends before it takes part.
    pub fn leave(&mut self) {
        self.asked = match self.asked.take() {
            // Not taken up yet: it ends as it drops, with the threads that find the other run.
            Some(Asked::Start(_)) => None,
            _ if self.networked => Some(Asked::Leave),
            asked => asked,
        };
    }

    /// The run's session for a game's own UI ([`SessionStatus`]), as the frame's turn left it,
    /// before its ticks. `None` alone, and for a server or client.
    pub fn session(&self) -> Option<&SessionStatus> {
        self.session.as_ref()
    }

    /// Puts `text` on the OS's clipboard once the frame ends. A headless or embedded run has none
    /// and warns (`ui.clipboard`). A focused text field copies and pastes itself, by Ctrl+C,
    /// Ctrl+X and Ctrl+V (Cmd on macOS).
    pub fn copy(&mut self, text: &str) {
        self.copied = Some(text.to_owned());
    }

    /// The app's window; `None` in a headless run.
    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// The window's physical pixels per logical pixel ([`Window::scale`]); 1 in a headless run.
    pub fn scale(&self) -> f64 {
        self.window.as_ref().map_or(1.0, Window::scale)
    }

    /// What [`save_state`](Self::save_state) last saved as file `name`, such as settings, for this
    /// user and machine and the app as [`App::new`] names it. `None` if nothing was saved, and in
    /// headless runs. `name`, such as `projects.json`, is a file in the OS's directory for the
    /// app's state, beside the dock's layout: on Windows, `%LOCALAPPDATA%\GGEngine\<app>`.
    ///
    /// Fails with `engine.state`, naming the file, if `name` isn't a file name alone, or the file
    /// exists but can't be read or isn't a regular file.
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

    /// Keeps `data` as file `name` for [`read_state`](Self::read_state) in later runs, written
    /// whole or not at all, so a crash leaves what the file held. Headless runs keep nothing.
    ///
    /// Fails with `engine.state`, naming the file, if `name` isn't a file name alone, or the file
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

    /// Keeps the user's state in `dir` instead of the OS's directory, for a test's context.
    #[cfg(all(test, feature = "dev"))]
    pub(crate) fn keep_state_in(&mut self, dir: PathBuf) {
        self.state = Some(dir);
    }

    /// The path of state file `name` ([`read_state`](Self::read_state)); `None` in a run that
    /// keeps none.
    fn state_path(&self, name: &str) -> Result<Option<PathBuf>> {
        let alone = Path::new(name).file_name() == Some(std::ffi::OsStr::new(name));
        if !alone {
            let message =
                format!("{name:?} isn't a file's name alone, which the app's state takes");
            return Err(Error::new("engine.state", message));
        }
        Ok(self.state.as_ref().map(|dir| dir.join(name)))
    }

    /// What's held down and where the cursor is, for layers that poll rather than follow events.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// Notes what `event` holds down or lets go, for tests that skip the stack.
    #[cfg(test)]
    pub(crate) fn record_input(&mut self, event: &Event<'_>) {
        self.input.record(event);
    }

    /// Whether layers want the cursor grabbed ([`set_cursor_grabbed`](Self::set_cursor_grabbed)),
    /// whether or not it's grabbed now.
    pub fn cursor_grabbed(&self) -> bool {
        self.cursor_grabbed
    }

    /// Grabs the cursor, for looking about with the mouse as first-person games do, or lets it
    /// go. Grabbed, it's hidden and held in the window (locked in place on Windows, macOS and
    /// Wayland, kept inside on X11), so the raw motion, [`Event::MouseMotion`], looks about
    /// however far it goes.
    ///
    /// It's held while the window has focus, and in a dockspace while the view has the keys; it
    /// lets go as either goes and returns with it. In a dockspace, a grab that starts with the
    /// cursor off the view first moves it to the view's center, where the platform can. Headless
    /// runs have no cursor. In a game the editor plays, the editor grabs its own window's while
    /// Play gives the game its input, letting go at Escape or Stop and grabbing again at a click
    /// in the view. Applied as the next frame starts, so the layer that asks last has its way.
    pub fn set_cursor_grabbed(&mut self, grabbed: bool) {
        self.cursor_grabbed = grabbed;
    }

    /// The cap on frames a second a windowed run renders ([`set_max_fps`](Self::set_max_fps));
    /// `None` while uncapped.
    pub fn max_fps(&self) -> Option<NonZeroU32> {
        self.max_fps
    }

    /// Caps a windowed run's frames at `max` a second, or uncaps with `None`, as a game's
    /// settings menu does. Before each frame the main loop waits out the rest of the frame's
    /// share of a second, counted from when the last was due, so the rate holds though a wait
    /// ends late. Once frames fall a whole share behind, it counts from the frame's start, so no
    /// burst follows a slow frame. Ticks go on in real time however fast frames come.
    ///
    /// `--max-fps` sets it at the start; in a development build the title bar's settings
    /// ([`Ui::settings`](crate::Ui::settings)) set it and keep it for the user, but in an app that
    /// embeds a game, where they set [`game_max_fps`](Self::game_max_fps). Headless runs (as fast
    /// as they can, or `--real-time`) ignore it, and embedded ones take their tool's cap
    /// ([`gg_api::Look::max_fps`]).
    pub fn set_max_fps(&mut self, max: Option<NonZeroU32>) {
        self.max_fps = max;
    }

    /// The cap on frames a second the title bar's settings set on the game an app shows
    /// ([`App::embeds_game`](crate::App::embeds_game)), which the app sends the game in its look
    /// ([`gg_api::Look::max_fps`]); `None` while uncapped.
    #[cfg(feature = "dev")]
    pub fn game_max_fps(&self) -> Option<NonZeroU32> {
        self.dev.game_max_fps
    }

    /// Sets [`game_max_fps`](Self::game_max_fps), as when another client caps the game.
    #[cfg(feature = "dev")]
    pub fn set_game_max_fps(&mut self, max: Option<NonZeroU32>) {
        self.dev.game_max_fps = max;
    }

    /// Ends the run before its next frame, as closing its window does when no layer handles that;
    /// for a game's Quit. Embedded in a tool, as in the editor's Play, it stops a copy of the world
    /// as edited that plays instead, as the tool API's `stop` does.
    pub fn exit(&mut self) {
        self.exiting = true;
    }

    /// Whether a layer has asked the run to end, by [`exit`](Self::exit) or [`fail`](Self::fail).
    pub fn exiting(&self) -> bool {
        self.exiting
    }

    /// Takes back an [`exit`](Self::exit), but not a [`fail`](Self::fail), for
    /// [`api::stop_at_exit`](crate::api::stop_at_exit). Returns whether it took one back.
    #[cfg(feature = "dev")]
    pub(crate) fn take_back_exit(&mut self) -> bool {
        let taken = self.exiting && !self.failed;
        if taken {
            self.exiting = false;
        }
        taken
    }

    /// Like [`exit`](Self::exit), but logs `error` at once by its code. For what a layer can't go
    /// on without, such as a script or scene that doesn't load, instead of a panic. The layers
    /// still detach and the run logs `engine.perf`, but not `Exited cleanly`, and its status is 1.
    /// A replay logs the error too; its status says whether its hashes matched.
    pub fn fail(&mut self, error: Error) {
        error!(code = error.code(), "{error:#}");
        self.exiting = true;
        self.failed = true;
    }

    /// The paths of the files the game's package holds
    /// ([`App::from_package`](crate::App::from_package), or a project's game built without
    /// `dev`), in the order of the paths' bytes. Each is joined to the asset root as layers name
    /// files, for [`Scripts::load`](crate::Scripts::load),
    /// [`load_scene_file`](Self::load_scene_file) and adds such as
    /// [`add_texture`](Self::add_texture). None for a run that reads the disk. A player loads a
    /// game's scripts by them, running a game without its code.
    pub fn package_files(&self) -> Vec<PathBuf> {
        self.files.paths()
    }

    /// Shows `dialog`, the OS's file dialog, modal over the main window, and returns the path
    /// chosen. `None` if cancelled, or if there's no window (headless runs and tests). For
    /// tools' menus, from [`Layer::ui`]. Frames pause while it's open, the render thread
    /// finishing the frame it has. No frame's time counts the pause: the next update gets none,
    /// as the first after a restore from minimized does.
    #[cfg(feature = "dev")]
    pub fn pick_file(
        &mut self,
        dialog: &crate::dialog::FileDialog<'_>,
    ) -> Option<std::path::PathBuf> {
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
