//! What apps build on. An app's `main` starts the engine; there's no entry-point crate.
//!
//! - The main loop ticks at a fixed rate whatever the frame rate. Window, input, and actions that
//!   name what a game does with it.
//! - Layers change the world at its ticks. A game's own UI draws on the view's canvas, laid out
//!   from UI files of named nodes; keys, pads and the mouse move its focus.
//! - The world: entities and components, drawn between ticks. Each frame's render data goes to
//!   `gg-renderer` on its own thread, a frame behind. The job system splits a query's rows across
//!   threads.
//! - Reflection describes component types so tools reach them by name; the Inspector edits them.
//! - Scripts: components that act at ticks.
//! - Physics, 2D and 3D, through rapier; its state is a world resource. [`World::cast_ray`] and
//!   [`World::cast_shape`] find what a ray or a moving shape meets in 3D.
//! - Random numbers a seed draws alike on every machine ([`Rng`]), and logging to the run's JSON
//!   log through [`tracing`].
//! - Navigation: a navmesh baked from static colliders, paths over it, and agents that walk them
//!   round each other.
//! - Animation: flipbooks and tweens. Particle emitters, whose particles the frames emit and own.
//! - Scenes and prefabs: entities as canonical text. Worlds of cells, each a scene file, come in
//!   around viewers and go out as text as each tick ends.
//! - Operations: authoring changes with inverses, in a log that undoes and redoes them.
//! - Recordings: a run's tick commands. They replay without a GPU to the same world, naming where
//!   one diverges.
//! - Per-frame measures for the log and the Performance panel. The grid a tool's view shows in an
//!   axis plane.
//! - Assets: images as textures; glTF models, whose nodes spawn as entities, with
//!   metallic-roughness materials the world's lights light; built-in [`Shape`]s, models without a
//!   file; a [`Tint`] per entity over the colors it draws; fonts baked into fields that draw text
//!   crisp at any size, shaped and laid out.
//! - Audio through kira: sounds ticks queue and layers play, panned and faded as the view hears
//!   them (a 2D view by where they show, a perspective camera by direction and distance); music,
//!   decoded as it plays, looping, one at a time; volume buses.
//! - Lockstep between two peers, each tick run on both peers' inputs.
//! - The pipeline cache's file, which windowed runs keep.
//!
//! Development builds (`dev`) add the project's manifest and assets' ID files; hot reload of
//! shaders, textures, models, fonts, prefabs, UIs, sounds and music; the OS's file dialogs; and
//! egui's UI, which layers draw through `Ui`: the dock, its title bars, the Inspector and
//! Performance panels, and a game's [`DebugOverlays`].
//!
//! A game in one file, a square turning before a camera. [`prelude`] names what most games use;
//! `cargo gg new` makes a game like it, its world in a scene file.
//!
//! ```no_run
//! use std::process::ExitCode;
//!
//! use gg_engine::Camera;
//! use gg_engine::math::DVec3;
//! use gg_engine::prelude::*;
//!
//! fn main() -> ExitCode {
//!     gg_engine::run(App::new("Hello").layer(Hello).debug_overlays())
//! }
//!
//! struct Hello;
//!
//! impl Layer for Hello {
//!     fn attach(&mut self, cx: &mut Context) {
//!         let projection = Projection::Orthographic {
//!             height: 4.0,
//!             near: 0.1,
//!             far: 100.0,
//!         };
//!         let camera = Camera {
//!             projection,
//!             primary: true,
//!         };
//!         let above = Transform {
//!             translation: DVec3::Z * 10.0,
//!             ..Transform::IDENTITY
//!         };
//!         let world = cx.world_mut();
//!         world.spawn((above, camera));
//!         world.spawn((Transform::IDENTITY, Sprite::default()));
//!     }
//!
//!     fn tick(&mut self, world: &mut World, _: Tick) {
//!         let turn = Quat::from_rotation_z(Tick::DT.as_secs_f32());
//!         for (at, _) in world.query_mut::<(&mut Transform, &Sprite)>() {
//!             at.rotation = (turn * at.rotation).normalize();
//!         }
//!     }
//! }
//! ```

// For the derives' code, which names `::gg_engine`.
extern crate self as gg_engine;

mod actions;
mod animation;
// The tool API's viewpoints, for tools that show the game's view, like the editor.
#[doc(hidden)]
pub mod api;
mod args;
#[cfg(feature = "dev")]
mod asset_watch;
mod assets;
mod audio;
#[cfg(feature = "dev")]
mod bounds;
mod camera;
mod canvas;
mod cells;
// `dev` alone has the UI.
#[cfg(feature = "dev")]
mod chrome;
#[doc(hidden)]
pub mod cook;
mod cooked;
// Without `dev`'s UI, `none.rs` holds a layer that does nothing.
#[cfg(feature = "dev")]
mod debug_overlays;
#[cfg(not(feature = "dev"))]
#[path = "debug_overlays/none.rs"]
mod debug_overlays;
#[cfg(feature = "dev")]
pub mod dialog;
mod dirs;
// Without `dev`'s UI, `none.rs` holds the dock's items games name, doing nothing.
#[cfg(feature = "dev")]
pub mod dock;
#[cfg(not(feature = "dev"))]
#[path = "dock/none.rs"]
pub mod dock;
mod draw;
pub mod ecs;
mod edits;
mod environment;
mod event;
mod font;
mod frame_rate;
#[cfg(feature = "dev")]
mod game_scale;
mod game_ui;
mod gltf;
pub mod grant;
pub mod grid;
#[cfg(feature = "dev")]
mod hot_reload;
mod image;
mod input;
// Installing the OS's Ctrl+C handler needs unsafe code, allowed in this module alone.
#[allow(unsafe_code)]
mod interrupt;
// Without `dev`'s UI, `none.rs` holds the Inspector's items games name, doing nothing.
#[cfg(feature = "dev")]
pub mod inspector;
#[cfg(not(feature = "dev"))]
#[path = "inspector/none.rs"]
pub mod inspector;
mod interpolation;
// For tests and the cooker.
#[doc(hidden)]
pub mod jobs;
mod layer;
mod lens;
mod light;
mod loader;
mod luau;
mod main_loop;
// The process's memory counts come from C functions on Windows and macOS: unsafe code, allowed in
// this module alone.
#[allow(unsafe_code)]
mod memory;
mod model;
mod name;
mod navigation;
mod net;
mod opaque;
mod owner;
mod package;
mod pads;
mod particles;
// subsecond patches the game's code only through unsafe code, allowed in this module alone.
#[cfg(feature = "dev")]
#[allow(unsafe_code)]
mod patch;
mod perf;
mod physics;
mod pipeline_cache;
mod placed;
mod prefab;
// For tests, which replay recordings.
#[doc(hidden)]
pub mod recording;
pub mod reflect;
mod render_thread;
mod renderer_components;
mod rng;
mod scene_change;
mod scene_files;
mod script;
mod shape;
mod skins;
mod sky;
mod snapshot;
mod sync_test;
#[cfg(test)]
mod testing;
mod tint;
mod transform;
// `dev` alone has the UI.
#[cfg(feature = "dev")]
mod view_input;
// `dev` alone has the UI.
#[cfg(feature = "dev")]
mod viewports;
mod window;
// Windows' own frame around a window whose title bar the UI draws needs a window procedure in
// front of winit's. Installing it is unsafe, allowed in this module alone.
#[cfg(all(target_os = "windows", feature = "dev"))]
#[allow(unsafe_code)]
mod windows_frame;
// An owner-only file, such as the tool API's session file, needs its own security descriptor on
// Windows. Passing it is unsafe, allowed in this module alone.
#[cfg(all(target_os = "windows", feature = "dev"))]
#[allow(unsafe_code)]
mod windows_private;
// The main thread's wait for the render thread answers its windows' messages meanwhile. That's
// unsafe, allowed in this module alone.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows_wait;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use package::{Files, Package};

pub use actions::{Actions, Binding, Change, Control, Controls, Source, Toward};
pub use animation::{
    AnimationParam, AnimationState, AnimationStates, Animator, BlendPoint, Curve, Ease, Flipbook,
    ParamCondition, ParamTest, ParamValue, Play, Playback, StateMotion, StateTransition, Tween,
    TweenValue, Tweens, step_animation,
};
pub use audio::{Bus, Music, Place, Sound};
pub use camera::{
    Camera, CameraAction, FlyAction, FlyCameraController, OrthographicCameraController,
};
pub use canvas::{CANVAS_HEIGHT, Canvas};
pub use cells::{Cell, Cells, Streamed, Viewer};
pub use debug_overlays::DebugOverlays;
#[doc(inline)]
pub use dock::Ui;
#[doc(inline)]
pub use ecs::{Bundle, Component, Entity, EntityId, Parent, Query, Resource, World};
/// The UI library [`Layer::ui`] draws with. Apps use it from here to share the engine's version.
/// A build without `dev` has no UI but keeps it for its types.
pub use egui;
pub use environment::{Adaptation, Environment};
pub use event::{Event, Key, MouseButton, Pad, PadButton, Scroll, Stick, Trigger};
pub use font::{Align, Font, Text, TextStyle};
pub use game_ui::{GameUi, UiEvent, UiValue};
/// The engine's vectors, matrices and quaternions. Apps use them from here to share its version.
pub use gg_core::math;
/// The engine's error, with its stable code, and its result.
pub use gg_core::{Error, Result};
/// Tracy zones, plots and frame marks. They compile to nothing without the `tracy` feature. Apps
/// use them from here: `let _zone = gg_engine::zone!();`.
pub use gg_core::{plot, profile, zone};
/// A mesh with its shader and parameters, their types erased so an entity can have one. The
/// engine draws it with the world.
pub use gg_renderer::MeshDraw;
/// What layers add and draw through their [`Context`]: sprites, sprite sheets, circles, meshes,
/// shaders for their vertex types (each added with its blend), textures and samplers. Also the
/// view's viewpoint and projection (a [`Camera`] has one too) and its place in the window; an
/// [`Environment`]'s tonemapper, antialiasing, grade, ambient occlusion, fog and reflections; the
/// least render scale ([`Context::set_render_scale`]); and what's still streaming to the GPU
/// ([`Context::streaming`]).
pub use gg_renderer::{
    AmbientOcclusion, Antialiasing, Blend, Circle, Fog, Grade, MIN_RENDER_SCALE, Mesh, Projection,
    Reflections, Sampler, Shader, Sprite, SpriteSheet, Streaming, Texture, Tonemapper, Viewpoint,
    Viewport,
};
#[doc(inline)]
pub use grant::Grant;
pub use input::Input;
#[doc(inline)]
pub use inspector::Inspector;
use layer::Stack;
pub use layer::{Commands, Context, Layer, Loading, Pick, Tick, Window};
pub use lens::{DepthOfField, Lens, MotionBlur};
pub use light::{Light, LightKind};
pub use luau::{Mods, Scripts};
use main_loop::run_headless;
pub use model::{Model, ModelNode, ModelRoot};
pub use name::Name;
pub use navigation::{NavAgent, NavMeshVolume, Navigation, step_navigation};
pub use net::{SessionPhase, SessionStatus};
pub use owner::Owner;
pub use particles::{EmitterShape, ParticleBlend, ParticleEmitter};
pub use perf::PerfPanel;
pub use physics::{
    BodyType, BoxCollider2D, BoxCollider3D, CapsuleCollider3D, Cast, CharacterController3D,
    CircleCollider2D, Contact, ContactEvent, Hit3D, Joint2D, Joint3D, JointKind2D, JointKind3D,
    MeshCollider3D, Physics2D, Physics3D, RigidBody2D, RigidBody3D, Shape3D, SphereCollider3D,
    step_physics,
};
pub use prefab::{Prefab, PrefabRoot};
use recording::Recorder;
pub use reflect::Reflect;
pub use rng::Rng;
pub use script::{Script, run_scripts};
pub use shape::Shape;
pub use sky::{Atmosphere, Sky};
use snapshot::Snapshot;
pub use tint::Tint;
/// Logging, as the engine logs: to the run's JSON log, which `gg logs` reads, and to its console.
/// Log with fields. A warning or error has a stable code: `warn!(code = "game.no_spawn", %name,
/// "no spawn point")`. Info and above reach the log without `RUST_LOG`.
pub use tracing;
pub use transform::Transform;

/// What most games name, for `use gg_engine::prelude::*;`.
pub mod prelude {
    pub use crate::math::{Mat4, Quat, Vec2, Vec3, Vec4};
    pub use crate::tracing::{debug, error, info, warn};
    pub use crate::{
        Actions, Align, App, Binding, Blend, Bus, Camera, Cast, Change, Commands, Component,
        Context, Control, Controls, Entity, Error, Event, Font, GameUi, Key, Layer, Mesh,
        MouseButton, PadButton, Place, Projection, Reflect, Rng, Scripts, Shader, Shape, Sound,
        Source, Sprite, Stick, TextStyle, Tick, Tint, Toward, Transform, World, step_animation,
        step_navigation, step_physics,
    };
}

const BUILD: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

/// An app's name and layers. Its `main` is
/// `gg_engine::run(gg_engine::App::new("Sandbox").layer(Sandbox::new()))`.
pub struct App {
    name: &'static str,
    stack: Stack,
    #[cfg(feature = "dev")]
    custom_title_bar: bool,
    /// The game view's panel. The dock then fills the main window.
    #[cfg(feature = "dev")]
    view_panel: Option<&'static str>,
    /// Where panels first dock, in the app's order.
    #[cfg(feature = "dev")]
    docked: Vec<(&'static str, dock::Docked)>,
    /// In points.
    window_size: [u32; 2],
    /// A PNG's bytes.
    icon: Option<&'static [u8]>,
    /// Seeds the world's IDs; `None` draws a random seed.
    seed: Option<u64>,
    /// Headless frames' spacing; `None` is a tick each.
    headless_step: Option<Duration>,
    /// What the layers' assets are named from ([`root`](Self::root)).
    root: Option<PathBuf>,
    project_dir: Option<PathBuf>,
    /// The manifest [`project`](Self::project) read, or why it couldn't.
    project: Option<Result<Manifest>>,
    /// The start scene, from the manifest or the package.
    start: Option<PathBuf>,
    /// The relay, from the manifest or the package. The run's own sessions meet through it unless
    /// `--relay` names another.
    relay: Option<String>,
    /// Set by [`from_package`](Self::from_package).
    from_package: bool,
    /// Where the run reads files under its asset root: the package once opened, else the disk.
    files: Files,
    /// `--mods`'s folder, in place of the installed mods.
    mods: Option<PathBuf>,
    /// When [`run`] began. The run's load is timed from it.
    started: Option<Instant>,
    /// How sessions the layers host and join start ([`Context::host`]). The run's arguments fill
    /// it in, but for [`predicting`](Self::predicting).
    own_sessions: layer::OwnSessions,
    /// Set by [`embeds_game`](Self::embeds_game).
    embeds_game: bool,
}

impl App {
    /// `name` titles the window and names the log file.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            stack: Stack::default(),
            #[cfg(feature = "dev")]
            custom_title_bar: false,
            #[cfg(feature = "dev")]
            view_panel: None,
            #[cfg(feature = "dev")]
            docked: Vec::new(),
            window_size: [1280, 720],
            icon: None,
            seed: None,
            headless_step: None,
            root: None,
            project_dir: None,
            project: None,
            start: None,
            relay: None,
            from_package: false,
            files: Files::default(),
            mods: None,
            started: None,
            own_sessions: layer::OwnSessions::default(),
            embeds_game: false,
        }
    }

    /// Has sessions the layers start ([`Context::host`], [`Context::join`]) predict the other
    /// peer's inputs and roll back where a prediction missed, like `--rollback` for a session its
    /// flags start. Both runs of one build agree, as a session's peers must. Without it, they go in
    /// lockstep.
    #[must_use]
    pub fn predicting(mut self) -> Self {
        self.own_sessions.predicting = true;
        self
    }

    /// Names the app's source by its digest, `env!("GG_SOURCE")`, which a build script running
    /// `gg_api::build::emit()` sets (gg-api's `build` says what it reads). A session's hello names
    /// the build by this digest and the package's, so runs of one source and package join whatever
    /// OS or machine built them (ARCHITECTURE.md's Networking). A run reading files from disk, as a
    /// development build does without `--package`, is named by its source alone. Without a source,
    /// the executable's digest names the app, and no other OS or machine's build shares it.
    ///
    /// # Panics
    ///
    /// - If `digest` isn't 16 hex digits, as `GG_SOURCE` is.
    #[must_use]
    pub fn source(mut self, digest: &'static str) -> Self {
        assert!(
            digest.len() == 16 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "a source's digest is 16 hex digits, as `GG_SOURCE` is, not {digest:?}"
        );
        self.own_sessions.build.source = Some(digest);
        self
    }

    /// Gives the window the icon `png`, a PNG's bytes, such as `include_bytes!(env!("GG_ICON"))`
    /// (gg-api's `build` names it from the manifest). Only Windows and X11 show a window's own
    /// icon. A PNG that doesn't decode warns `engine.icon`, and the window opens without it.
    #[must_use]
    pub fn icon(mut self, png: &'static [u8]) -> Self {
        self.icon = Some(png);
        self
    }

    /// Names each asset the layers add from a file by its path relative to `root`, with `/`
    /// between parts, such as `assets/logo.png`. `root` is the game's directory,
    /// `env!("CARGO_MANIFEST_DIR")`. Scenes, snapshots, recordings and the tool API name textures
    /// this way, so another run or machine finds them. Without it, or for a file outside it, an
    /// asset is named by the path the layer gave.
    ///
    /// In a development build, each texture, model, font and prefab under `root` has an ID in an ID
    /// file beside it. A run serving the tool API writes missing ones, except a lockstep joiner,
    /// whose host writes them. See ARCHITECTURE.md's Project state.
    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// Makes the app the game of the project whose manifest, `project.ron`, is in `dir`: the game's
    /// directory, `env!("CARGO_MANIFEST_DIR")` (ARCHITECTURE.md's Project state). The layers'
    /// assets are named from its asset root, like [`root`](Self::root).
    /// [`Context::load_start_scene`] loads its start scene. The tool API's session file names the
    /// manifest, so tools inside the project find the run. A manifest that doesn't read ends the
    /// run as it starts, with its error.
    ///
    /// A build without `dev` reads no manifest: shipping builds run from cooked data, the game's
    /// package `tools/cook` writes. It holds the asset root's files and names the start scene. The
    /// package is `<executable>.ggpk` beside the executable, in a macOS bundle's
    /// `Contents/Resources`, or `--package`'s, which a development build can run from too.
    #[must_use]
    pub fn project(mut self, dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        self.project_dir = Some(dir.to_owned());
        #[cfg(feature = "dev")]
        {
            let manifest = Manifest::read(dir);
            if let Ok(manifest) = &manifest {
                self.root = Some(manifest.root.clone());
                self.start = Some(manifest.start.clone());
                self.relay.clone_from(&manifest.relay);
            }
            self.project = Some(manifest);
        }
        self
    }

    /// Runs the app from a game's package in any build, as the reference player runs a game
    /// without its code. The package is `--package`'s, else the one beside the executable (see
    /// [`project`](Self::project)); without one, the run fails as it starts (`engine.no_package`).
    /// Assets are named by the package's IDs, from its asset root under the package's directory,
    /// or the project's for a project's game. [`Context::load_start_scene`] loads its start scene;
    /// [`Context::package_files`] lists its files.
    #[must_use]
    pub fn from_package(mut self) -> Self {
        self.from_package = true;
        self
    }

    /// Like `--package path`, for tests, whose arguments are their harness's.
    ///
    /// Fails like [`Package::open`].
    #[doc(hidden)]
    pub fn package(mut self, path: impl AsRef<Path>) -> Result<Self> {
        self.open_package(Some(path.as_ref()))?;
        Ok(self)
    }

    /// Opens the game's package for the files under the asset root: `given` (`--package`), else,
    /// for a [`from_package`](Self::from_package) app or a project's game without `dev`, the one
    /// beside the executable, unless [`package`](Self::package) gave one. The package names the
    /// start scene and the asset root, under the project's directory, or its own for an app that
    /// isn't a project's game.
    ///
    /// Fails like [`Package::open`], and with `engine.no_package` for an app that runs from a
    /// package without one beside it.
    fn open_package(&mut self, given: Option<&Path>) -> Result<()> {
        let shipped = !cfg!(feature = "dev") && self.project_dir.is_some();
        let path = match given {
            Some(path) => path.to_owned(),
            None if (self.from_package || shipped) && self.files.packaged().is_none() => {
                let path =
                    std::env::current_exe().map(|exe| package_beside(&exe, std::env::consts::OS));
                match path {
                    Ok(path) if path.is_file() => path,
                    path => {
                        let path = path.map_or_else(
                            |_| "<executable>.ggpk".to_owned(),
                            |path| path.display().to_string(),
                        );
                        let runs = if self.from_package {
                            "the app runs from a game's package"
                        } else {
                            "a build without `dev` runs from its game's package"
                        };
                        let message = format!(
                            "{runs}, {path}, which isn't there: `tools/cook` writes it, or \
                             `--package` names another"
                        );
                        return Err(Error::new("engine.no_package", message));
                    }
                }
            }
            None => return Ok(()),
        };
        let package = Arc::new(Package::open(&path)?);
        let dir = (self.project_dir.clone())
            .or_else(|| path.parent().map(Path::to_owned))
            .unwrap_or_default();
        let root = package::under(&dir, package.assets());
        self.start = package.start().map(|start| package::under(&dir, start));
        self.relay = package.relay().map(str::to_owned);
        self.files = Files::package(package, root.clone());
        self.root = Some(root);
        info!(path = %path.display(), "Running from the game's package");
        Ok(())
    }

    /// `None` for an app that isn't a project's game, or whose manifest didn't read.
    fn manifest(&self) -> Option<&Manifest> {
        self.project.as_ref()?.as_ref().ok()
    }

    /// Draws the world's IDs from `seed`, not a random one, so IDs and what follows from them (such
    /// as the draw order of entities at one z) repeat each run. For tests.
    #[doc(hidden)]
    #[must_use]
    pub fn seeded(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Spaces headless frames `dt` apart, as their measures take them, rather than a tick each. The
    /// simulation steps as in windowed runs, so frames draw the world between ticks. A `dt` under a
    /// tick runs at most one tick a frame, however long one takes. For tests.
    #[doc(hidden)]
    #[must_use]
    pub fn headless_step(mut self, dt: Duration) -> Self {
        self.headless_step = Some(dt);
        self
    }

    /// Adds a layer above the layers added before it, and below every overlay.
    #[must_use]
    pub fn layer(mut self, layer: impl Layer + 'static) -> Self {
        self.stack.push_layer(layer);
        self
    }

    /// Adds an overlay above everything added before it.
    #[must_use]
    pub fn overlay(mut self, overlay: impl Layer + 'static) -> Self {
        self.stack.push_overlay(overlay);
        self
    }

    /// Adds the [`PerfPanel`] and [`Inspector`] overlays, in that order: for a tool with a
    /// [custom title bar](Self::custom_title_bar), whose View menu closes them. A game takes
    /// [`debug_overlays`](Self::debug_overlays).
    #[must_use]
    pub fn dev_panels(self) -> Self {
        self.overlay(PerfPanel).overlay(Inspector::default())
    }

    /// Adds the [`DebugOverlays`] overlay, which a development build's key below Esc opens: the
    /// Performance, Entities and Inspector panels among them. Add it last, so it's on top.
    #[must_use]
    pub fn debug_overlays(self) -> Self {
        self.overlay(DebugOverlays::default())
    }

    /// Has the UI draw the app's window frames instead of the OS: for tools, which want the room.
    /// Games keep the OS's frame, as does a build without `dev`. On macOS the OS keeps its buttons
    /// and edges.
    ///
    /// - The layers' [`Ui::menu`] menus sit behind a button in the main window's title bar.
    /// - Torn-off panels' tab bars are their windows' title bars.
    /// - The dock adds a View menu after the layers' menus. It opens and closes panels and saves,
    ///   loads and resets the layout.
    /// - Tabs get a menu that closes their panels.
    /// - Title bar buttons hide and show the panels left of, below and right of the game's view
    ///   ([`Ui::panel`]).
    #[must_use]
    pub fn custom_title_bar(self) -> Self {
        Self {
            #[cfg(feature = "dev")]
            custom_title_bar: true,
            ..self
        }
    }

    /// Marks a tool that shows a game it embeds, as the editor shows its Play. Its own frame rate
    /// limit starts at Match monitor, presenting at the display's refresh rate as `--vsync` does,
    /// unless its title bar's settings kept another. They limit the game's too.
    ///
    /// The embedded game renders a frame each time the tool takes one, so the tool's rate is the
    /// game's ceiling.
    #[must_use]
    pub fn embeds_game(self) -> Self {
        Self {
            embeds_game: true,
            ..self
        }
    }

    /// Shows the game's view in its own panel, `name`, docked and floated like the layers' panels.
    /// The dock fills the main window, without games' central area: for tools. The view fills the
    /// dock until panels dock beside it, and never goes into a window of its own.
    ///
    /// The layers get:
    ///
    /// - mouse presses and the wheel over the view, and the drags those start;
    /// - keys while no egui widget has focus, no egui modal is open and the view shows, if the view
    ///   has focus or the cursor is over it (or [`Ui::over_view`]'s content) while egui drags none
    ///   of its widgets;
    /// - modifier keys pressed anywhere while no egui widget has focus, and every release.
    ///
    /// A press on the view or [`Ui::over_view`]'s content gives it focus, like [`Ui::focus_view`];
    /// a press elsewhere takes it. It has focus as the app starts; [`Ui::view_focused`] tells. A
    /// build without `dev` changes nothing: the view fills the window, its input to the layers.
    #[must_use]
    pub fn view_panel(self, name: &'static str) -> Self {
        let _ = name;
        Self {
            #[cfg(feature = "dev")]
            view_panel: Some(name),
            ..self
        }
    }

    /// Opens the window `width` by `height` points inside its frame (default 1280 by 720), capped
    /// at its monitor's size. Headless and embedded runs ignore it.
    #[must_use]
    pub fn window_size(self, width: u32, height: u32) -> Self {
        Self {
            window_size: [width, height],
            ..self
        }
    }

    /// Docks `panel` where `docked` says when it first shows in a layout that lacks it: on the
    /// app's first run, after the View menu's Reset layout, or with a layout saved before the app
    /// had it. A build without `dev` ignores it.
    ///
    /// - Beside a panel, it takes the share of that panel's place `docked` gives. Tabbed with one,
    ///   the showing tab stays showing.
    /// - Panels first showing in one frame dock in the app's order, whatever order the layers show
    ///   them in.
    /// - A target the layout lacks stands for its own target, and so on. A panel showing before its
    ///   target docks beside the target's target, and stays there once the target shows.
    /// - A panel whose target floats, or comes to none, floats, like panels the app doesn't dock.
    ///
    /// The editor docks three panels around its view: the Inspector takes a quarter of the window,
    /// Entities a fifth of the rest, and Performance the bottom half of the Inspector's:
    ///
    /// ```
    /// use gg_engine::App;
    /// use gg_engine::dock::Docked;
    ///
    /// let app = App::new("Editor")
    ///     .custom_title_bar()
    ///     .view_panel("Viewport")
    ///     .dock("Inspector", Docked::RightOf("Viewport", 0.25))
    ///     .dock("Entities", Docked::LeftOf("Viewport", 0.2))
    ///     .dock("Performance", Docked::Below("Inspector", 0.5));
    /// ```
    #[must_use]
    pub fn dock(self, panel: &'static str, docked: dock::Docked) -> Self {
        let _ = (panel, docked);
        Self {
            #[cfg(feature = "dev")]
            docked: [self.docked, vec![(panel, docked)]].concat(),
            ..self
        }
    }
}

/// Runs `app` until its window closes or `--frames` runs out, or replays a recording
/// (`--replay`). Parses the flags every app shares (`--help`) and starts logging.
///
/// Exits 0 on success; 1 on an engine or layer error ([`Context::fail`]), a Vulkan validation
/// message, or a replay that diverges; 2 on bad arguments.
pub fn run(app: App) -> ExitCode {
    let started = Instant::now();
    let mut argv = std::env::args_os();
    let program = argv
        .next()
        .and_then(|path| Some(Path::new(&path).file_stem()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| app.name.to_lowercase());
    run_args(app, started, &program, argv)
}

/// Like [`run`], with `args` in place of the process's arguments after its name.
///
/// For tests, whose own are their harness's, and apps that take their own flags first, like the
/// editor. `--help`'s usage names the program after the app.
pub fn run_with(app: App, args: impl IntoIterator<Item = impl Into<OsString>>) -> ExitCode {
    let program = app.name.to_lowercase();
    let args = args.into_iter().map(Into::into);
    run_args(app, Instant::now(), &program, args)
}

/// The body of [`run`] and [`run_with`].
fn run_args(
    mut app: App,
    started: Instant,
    program: &str,
    args: impl IntoIterator<Item = OsString>,
) -> ExitCode {
    let args = match args::parse(args) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{}", args::usage(program));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!(
                "error[{}]: {error}\n\n{}",
                error.code(),
                args::usage(program)
            );
            return ExitCode::from(2);
        }
    };

    // An embedded run's tool reads events through the tool API's `logs`; stderr would show each
    // twice.
    let log = gg_core::log::init(app.name, &log_dir(app.name), !args.embedded);
    info!(
        version = env!("CARGO_PKG_VERSION"),
        app = app.name,
        build = BUILD,
        source = app.own_sessions.build.source,
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        cpus = std::thread::available_parallelism().map_or(0, |cpus| cpus.get()),
        threads = args.threads,
        pid = std::process::id(),
        "Starting GGEngine"
    );
    if let Some(path) = &log {
        info!(path = %path.display(), "Writing the JSON log");
    }
    debug!(
        headless = args.headless,
        embedded = args.embedded,
        supervised = args.supervised,
        frames = args.frames,
        size = args.size.map(|[width, height]| format!("{width}x{height}")),
        capture = args.capture.map(|frame| frame.get()),
        gpu = args.gpu.as_deref(),
        validation = args.validation,
        record = args.record.as_ref().map(|path| path.display().to_string()),
        replay = args.replay.as_ref().map(|path| path.display().to_string()),
        sync_test = args.sync_test,
        rollback = args.rollback,
        link = args.link.map(|link| format!("{link:?}")),
        "Options"
    );

    if let Some(Err(error)) = &app.project {
        error!(code = error.code(), "{error:#}");
        return ExitCode::FAILURE;
    }
    // Before the replay too, whose layers load what they add from it.
    if let Err(error) = app.open_package(args.package.as_deref()) {
        error!(code = error.code(), "{error:#}");
        return ExitCode::FAILURE;
    }
    app.mods.clone_from(&args.mods);
    app.started = Some(started);
    // Before the replay too, which ticks on them as a run does.
    if let Err(error) = jobs::start(args.threads) {
        error!(code = error.code(), "{error:#}");
        return ExitCode::FAILURE;
    }
    // Before the replay too, whose ticks run again as a run's do.
    if args.sync_test {
        app.stack.sync_test();
    }
    // Before any renderer, which a replay doesn't need, and which a recording that can't be
    // written would keep from starting.
    if let Some(path) = &args.replay {
        return recording::replay(app, path, args.dump);
    }
    // Before the tool API starts: tools take a run serving it as started, so a snapshot it can't
    // restore must end it first.
    let snapshot = (args.restore.as_deref())
        .map(|path| Snapshot::read(path, app.name))
        .transpose();
    let recorder = args
        .record
        .as_deref()
        .map(|path| Recorder::create(path, app.name));
    let ran = snapshot.and_then(|snapshot| {
        if let Some(recorder) = recorder.transpose()? {
            app.stack.record(recorder);
        }
        app.own_sessions.build.files = app.files.clone();
        if let Some(net) = &args.net {
            let (code, names, rollback) = (args.code, app.stack.names(), args.rollback);
            let build = &app.own_sessions.build;
            let session =
                net::Session::begin(net, code, app.name, build, names, rollback, args.link)?;
            app.stack.session(session);
        }
        let own = &mut app.own_sessions;
        (own.app, own.layers) = (app.name, app.stack.names().to_vec());
        own.relay = args.relay.clone().or_else(|| app.relay.clone());
        own.link = args.link;
        own.recorded = args.record.is_some();
        // A server renders nothing, so serves no tool API, whose tools look at frames.
        if matches!(args.net, Some(args::Net::Serve(_))) {
            interrupt::watch();
            if args.supervised {
                interrupt::watch_stdin(|| {});
            }
            return main_loop::run_server(&mut app, &args, snapshot);
        }
        // While the renderer starts and the layers attach.
        font::bake_inter_ahead();
        // Before the renderer starts, so tools find a run that hangs starting.
        let player = args.net.as_ref().and_then(|net| match net {
            args::Net::Host(_) => Some(0),
            args::Net::Join(_) => Some(1),
            args::Net::Serve(_) | args::Net::Connect(_) => None,
        });
        let api = match api::start(
            app.name,
            app.stack.names(),
            log.as_deref(),
            player,
            app.manifest().map(|manifest| manifest.path.as_path()),
            args.api,
        ) {
            Ok(api) => api,
            // An embedded run's tool reaches it only through the API (its console is off), and a
            // stopped run plays only on the API's `play`. Either ends, saying why, rather than
            // running unseen or stopped for good.
            Err(error) if args.embedded || args.stopped => return Err(error),
            Err(error) => {
                let consequence = "tools can't reach this run";
                warn!(code = error.code(), "{error:#}; {consequence}");
                None
            }
        };
        if args.headless || args.embedded {
            // An embedded run's tool ends it.
            if args.headless {
                interrupt::watch();
            }
            // Their frames go on without events, so they need no waking.
            if args.supervised {
                interrupt::watch_stdin(|| {});
            }
            run_headless(&mut app, &args, api, snapshot)
        } else {
            window::run(&mut app, &args, api, snapshot)
        }
    });
    match ran {
        Ok(ran) => ran.report(started, &args, app.stack.failed()),
        Err(error) => {
            error!(code = error.code(), "{error:#}");
            ExitCode::FAILURE
        }
    }
}

/// What a run takes from its project's manifest ([`App::project`]). `scenes` holds the folders of
/// the scene files the tool API's `scene` opens and saves.
pub(crate) struct Manifest {
    pub(crate) path: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) start: PathBuf,
    #[cfg(feature = "dev")]
    pub(crate) relay: Option<String>,
    #[cfg(feature = "dev")]
    pub(crate) scenes: Vec<PathBuf>,
}

impl Manifest {
    /// Reads the manifest in `dir`.
    ///
    /// Fails like gg-api's `Project::read`, with `project.read` or `project.bad`, and with
    /// `engine.no_project` in a build without `dev`, which reads none.
    pub(crate) fn read(dir: &Path) -> Result<Self> {
        #[cfg(feature = "dev")]
        {
            let project = gg_api::Project::read(dir).map_err(|failure| {
                // Static codes, as the engine's are.
                let code = match failure.code.as_str() {
                    "project.read" => "project.read",
                    _ => "project.bad",
                };
                Error::new(code, failure.message)
            })?;
            Ok(Self {
                path: dir.join(gg_api::Project::FILE),
                root: project.assets_dir(dir),
                start: project.start_path(dir),
                scenes: project.scene_dirs(dir),
                relay: project.relay,
            })
        }
        #[cfg(not(feature = "dev"))]
        {
            let _ = dir;
            let message = "a build without `dev` reads no project's manifest; shipping builds \
                           start from cooked data";
            Err(Error::new("engine.no_project", message))
        }
    }
}

/// The UI's font family for Inter Bold, used by the Inspector's axis buttons and open to tools'
/// headings: `egui::FontFamily::Name(BOLD.into())`.
pub const BOLD: &str = "Bold";

/// egui in the engine's UI font, Inter:
///
/// - Regular for proportional text, its digits and minus sign tabular (a figure space wide), so
///   changing numbers hold still.
/// - Bold in the family [`BOLD`] names.
/// - Both fall back to egui's proportional fonts for glyphs Inter lacks, such as egui's icons.
///   Monospace text and every size stay egui's.
///
/// Collapsing headers are framed, as wide as their `Ui`, in the light and dark themes; the OS's
/// picks. The fonts are in the binary; egui parses a family's as it first lays out text in it.
/// egui's quit shortcut, Ctrl or Cmd+Q, is off: a game may use those keys, and apps quit from
/// their own menus.
#[cfg(feature = "dev")]
fn egui_context() -> egui::Context {
    let egui = egui::Context::default();
    egui.options_mut(|options| options.quit_shortcuts.clear());
    let mut fonts = egui::FontDefinitions::default();
    let fallbacks = fonts.families[&egui::FontFamily::Proportional].clone();
    let inter: [(egui::FontFamily, &str, &'static [u8]); 2] = [
        // Inter Regular 4.001, but its cmap maps 0 to 9 and U+2212 to their `tnum` glyphs, as
        // Blender's Inter does: egui shapes text without OpenType's optional features. Made with
        // fontTools: each of those code points, in each cmap subtable, set to the tnum lookups'
        // substitute, read from a copy of the font so its GSUB saves unchanged. The head table's
        // checksum and date are fontTools' as it saves.
        (egui::FontFamily::Proportional, "Inter", font::INTER),
        (
            egui::FontFamily::Name(BOLD.into()),
            "Inter Bold",
            include_bytes!("../assets/fonts/Inter/Inter-Bold.ttf"),
        ),
    ];
    for (family, name, data) in inter {
        let data = std::sync::Arc::new(egui::FontData::from_static(data));
        fonts.font_data.insert(name.to_owned(), data);
        let mut chain = vec![name.to_owned()];
        chain.extend(fallbacks.iter().cloned());
        fonts.families.insert(family, chain);
    }
    egui.set_fonts(fonts);
    egui.all_styles_mut(|style| style.visuals.collapsing_header_frame = true);
    egui
}

/// The engine's menu style: egui's, with items padded 8 points across and 4 down (egui's are 2 and
/// 0), as roomy as Fluent's and JetBrains', so the hover highlight frames an item's text. Rounded
/// 4 points, like theirs. For each menu the engine shows; submenus take their parent's style.
#[cfg(feature = "dev")]
pub(crate) fn menu_style(style: &mut egui::Style) {
    egui::menu::menu_style(style);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    let widgets = &mut style.visuals.widgets;
    for state in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        state.corner_radius = egui::CornerRadius::same(4);
    }
}

/// Directory `name` beside the executable, such as `target/debug/logs` under `cargo run`, or in
/// the working directory if the executable's path is unknown. It holds logs ([`log_dir`]) and, in
/// development builds, the tool API's session files and captures.
pub(crate) fn beside_exe(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Where app `name` ([`App::new`]) writes its JSON log:
///
/// - In a development build, `logs` beside the executable (`target/debug/logs` under `cargo run`),
///   where agents find it.
/// - Without `dev`, `logs` in the app's per-user state directory, as a shipped executable's
///   directory may be read-only (Program Files, a signed .app). Without the variables that
///   directory needs, beside the executable.
///
/// Public for tests, which read the log.
#[doc(hidden)]
pub fn log_dir(name: &str) -> PathBuf {
    if !cfg!(feature = "dev")
        && let Some(dir) = dirs::dir(name, dirs::Kind::State, std::env::consts::OS, |var| {
            std::env::var_os(var)
        })
    {
        return dir.join("logs");
    }
    beside_exe("logs")
}

/// The default package path ([`App::project`]) for the executable `exe` on `os`
/// ([`std::env::consts::OS`]): `<executable>.ggpk` beside it. On macOS, for an executable in a
/// bundle's `Contents/MacOS`, it's in `Contents/Resources`, as `gg ship` lays bundles out.
fn package_beside(exe: &Path, os: &str) -> PathBuf {
    let macos = exe
        .parent()
        .filter(|dir| os == "macos" && dir.ends_with("MacOS"));
    let contents = macos
        .and_then(Path::parent)
        .filter(|dir| dir.ends_with("Contents"));
    match (contents, exe.file_name()) {
        (Some(contents), Some(name)) => {
            contents.join("Resources").join(name).with_extension("ggpk")
        }
        _ => exe.with_extension("ggpk"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package sits beside its executable, named after it; on macOS, in a bundle's Resources.
    #[test]
    fn a_package_is_beside_its_executable_or_in_its_bundles_resources() {
        let bundled = Path::new("/Applications/Shards.app/Contents/MacOS/gg-shards");
        assert_eq!(
            package_beside(bundled, "macos"),
            Path::new("/Applications/Shards.app/Contents/Resources/gg-shards.ggpk")
        );
        for (exe, os, package) in [
            (
                bundled,
                "linux",
                "/Applications/Shards.app/Contents/MacOS/gg-shards.ggpk",
            ),
            (
                Path::new("/home/ada/shards/gg-shards"),
                "linux",
                "/home/ada/shards/gg-shards.ggpk",
            ),
            (
                Path::new("/Users/ada/MacOS/gg-shards"),
                "macos",
                "/Users/ada/MacOS/gg-shards.ggpk",
            ),
            (
                Path::new(r"C:\Games\Shards\gg-shards.exe"),
                "windows",
                r"C:\Games\Shards\gg-shards.ggpk",
            ),
        ] {
            assert_eq!(package_beside(exe, os), Path::new(package), "{os}");
        }
    }

    /// A run's sessions use the manifest's relay, or, run from a package, the package's (or none),
    /// like its start scene.
    #[cfg(feature = "dev")]
    #[test]
    fn a_run_takes_its_project_s_relay_from_its_manifest_or_its_package() {
        let scratch = crate::dirs::tests::Scratch::new("project-relay");
        let dir = &scratch.0;
        std::fs::create_dir_all(dir).expect("a directory");
        let project = gg_api::Project {
            game: "tested".to_owned(),
            assets: ".".to_owned(),
            scenes: vec!["scenes".to_owned()],
            start: "scenes/main.ron".to_owned(),
            relay: Some("manifest.example.com:7800".to_owned()),
            icon: None,
        };
        std::fs::write(dir.join(gg_api::Project::FILE), project.text()).expect("written");
        let app = App::new("Relayed").project(dir);
        assert_eq!(app.relay.as_deref(), Some("manifest.example.com:7800"));
        for relay in [Some("package.example.com:7800"), None] {
            let path = dir.join(format!("{}.ggpk", relay.is_some()));
            let writer = package::Writer::new(".", Some("scenes/main.ron"), relay);
            writer.write(&path).expect("written");
            let app = App::new("Relayed").project(dir).package(&path);
            let app = app.unwrap_or_else(|error| panic!("{error:#}"));
            assert_eq!(app.relay.as_deref(), relay);
        }
    }

    /// [`egui_context`]'s fonts, tabular figures, fallbacks and framed headers in both themes.
    #[cfg(feature = "dev")]
    #[test]
    fn the_ui_draws_in_inter_with_its_headers_framed() {
        let egui = egui_context();
        // egui makes its fonts as its first pass begins.
        let mut output = egui.run_ui(egui::RawInput::default(), |_| {});
        // As the renderer would have.
        output.textures_delta.clear();
        let icon = egui::special_emojis::GITHUB.to_string();
        egui.fonts_mut(|fonts| {
            for (family, font) in [
                (egui::FontFamily::Proportional, "Inter"),
                (egui::FontFamily::Name(BOLD.into()), "Inter Bold"),
            ] {
                let chain = fonts.definitions().families[&family].clone();
                assert_eq!(chain[0], font);
                assert!(chain.iter().any(|name| name == "egui-icons"), "{chain:?}");
                let in_font = |names: &Vec<String>| names.iter().any(|name| name == font);
                let characters = fonts.characters(&family);
                // The axes' letters, and the ellipsis ending menu items (Save layout as…) and
                // names cut short.
                let has = |c| characters.get(&c).is_some_and(in_font);
                assert!("XYZW…".chars().all(has), "{font} lacks one");
                let id = egui::FontId::new(14.0, family);
                assert!(fonts.has_glyphs(&id, &icon), "{font} falls back");
            }
            // Tabular: a number's width doesn't change with its digits, its sign or the figure
            // space in its place, nor with kerning, which Inter's proportional digits have.
            let font = egui::FontId::proportional(13.0);
            let mut width = |text: &str| {
                let white = egui::Color32::WHITE;
                fonts
                    .layout_no_wrap(text.to_owned(), font.clone(), white)
                    .size()
                    .x
            };
            for (zeros, numbers) in [
                (
                    "\u{2007}0000",
                    ["\u{2007}1111", "\u{2007}7474", "\u{2212}4170"],
                ),
                (
                    "\u{2007}0.00",
                    ["\u{2007}1.11", "\u{2007}7.41", "\u{2212}9.17"],
                ),
            ] {
                let figures = width(zeros);
                for number in numbers {
                    assert_eq!(width(number), figures, "{number}");
                }
            }
            let monospace = &fonts.definitions().families[&egui::FontFamily::Monospace];
            assert_eq!(monospace[0], "Hack");
        });
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let style = egui.style_of(theme);
            assert!(style.visuals.collapsing_header_frame, "{theme:?}");
        }
        let preference = egui.options(|options| options.theme_preference);
        assert_eq!(preference, egui::ThemePreference::System);
    }
}
