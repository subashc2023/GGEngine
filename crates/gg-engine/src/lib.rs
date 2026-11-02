//! What apps build on: the main loop, which steps the simulation at a fixed rate whatever the frame
//! rate, window and input, and the actions that name what a game does with it, and layers, which
//! change the world at its ticks, and their UI, a game's own on the view's canvas; the world of
//! entities and their components, which it draws between its ticks, handing each frame's render
//! data to `gg-renderer` on a thread of its own, a frame behind, and the job system, which a tick's
//! systems split a query's rows across;
//! reflection, which describes the components' types so that tools reach them by name, and the
//! Inspector, which edits them; scripts, components that act at ticks; 2D physics, through rapier,
//! whose state is a resource of the world; scenes and prefabs, the world's entities as canonical
//! text; operations, the authoring changes made to the world, each with its inverse, and their
//! log, which undoes and redoes them; recordings of runs, the commands their ticks applied, which
//! replay without a GPU to the same world, naming where one goes another way; and measuring each
//! frame, for the log and the Performance panel; the grid a tool's view shows in an axis plane;
//! loading what layers add, images as textures, glTF models, whose nodes spawn as entities, and
//! fonts, baked into fields that draw text crisp at any size, which it shapes and lays out; sounds,
//! which ticks queue and layers play, through kira, panned and faded by where they show in the view;
//! lockstep between two peers, each tick run on both peers' inputs; the pipeline cache's file,
//! which windowed runs keep;
//! and in development builds, behind `dev`, reading a game's project's manifest and its assets' ID
//! files, reloading games' shaders, textures, models, fonts and sounds as their files change, and
//! the OS's file dialogs.
//!
//! An app's `main` starts the engine; there is no separate entry-point crate.

// So that the derives' code, which names `::gg_engine`, works here too.
extern crate self as gg_engine;

mod actions;
mod api;
mod args;
#[cfg(feature = "dev")]
mod asset_watch;
mod assets;
mod audio;
mod camera;
mod canvas;
mod chrome;
// In development builds alone, as `dev` has it.
#[cfg(feature = "dev")]
mod dialog;
mod dirs;
mod dock;
mod draw;
mod ecs;
mod edits;
mod event;
mod font;
mod gltf;
pub mod grid;
// In development builds alone, as `dev` has it.
#[cfg(feature = "dev")]
mod hot_reload;
mod image;
mod input;
mod inspector;
mod interpolation;
mod jobs;
mod layer;
mod main_loop;
mod model;
mod name;
mod net;
mod pads;
// subsecond applies a patch of the game's code only through unsafe code: this module alone may use
// it, in development builds alone.
#[cfg(feature = "dev")]
#[allow(unsafe_code)]
mod patch;
mod perf;
mod physics;
mod pipeline_cache;
mod recording;
pub mod reflect;
mod render_thread;
mod renderer_components;
mod scene_files;
mod script;
mod snapshot;
// What tests share, in tests alone.
#[cfg(test)]
mod testing;
mod transform;
mod view_input;
mod viewports;
mod window;
// Windows' own frame around a window whose title bar the UI draws needs a window procedure in
// front of winit's, which only unsafe code installs: this module alone may use it.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows_frame;
// The main thread's wait for the render thread answers what's sent to its windows meanwhile, which
// only unsafe code does: this module alone may use it.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows_wait;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

pub use actions::{Actions, Binding, Change, Source, Toward};
pub use audio::{Place, Sound};
pub use camera::{
    Camera, CameraAction, FlyAction, FlyCameraController, OrthographicCameraController,
};
pub use canvas::{CANVAS_HEIGHT, Canvas};
#[cfg(feature = "dev")]
pub use dialog::FileDialog;
pub use dock::{Docked, Ui};
pub use ecs::{
    Bundle, Component, Difference, Entities, Entity, EntityId, Log, Operation, Parent, Pose, Query,
    QueryIter, QueryIterMut, ReadOnlyQuery, Resource, Saved, World, WorldHash, Writable,
    read_scene, write_scene,
};
/// The UI library layers draw with, in [`Layer::ui`]; apps use it from here, so that they
/// share the engine's version.
pub use egui;
pub use event::{Event, Key, MouseButton, Pad, PadButton, Scroll, Stick, Trigger};
pub use font::{Align, Font, Text, TextStyle};
/// The engine's vectors, matrices and quaternions; apps use them from here, so that they share
/// the engine's version.
pub use gg_core::math;
/// The error the engine's calls that can fail return, with its stable code, and their result.
pub use gg_core::{Error, Result};
/// The engine's profiling: zones, plots and frame marks for Tracy, which compile to nothing unless
/// the `tracy` feature is on. Apps use them from here, such as `let _zone = gg_engine::zone!();`.
pub use gg_core::{plot, profile, zone};
/// A mesh to draw with a shader and its parameters, without their types, so that an entity can
/// have one, which the engine draws as it draws the world.
pub use gg_renderer::MeshDraw;
/// What layers draw, which they add and draw through their [`Context`]: sprites, and the sprite
/// sheets whose cells they show; circles; meshes; shaders for their vertex types, each added with
/// how it blends; the textures those shaders and sprites sample, and the samplers they sample them
/// with; the viewpoint the game's view sees them from, and its projection, which a [`Camera`] has
/// too; and where that view is in the window.
pub use gg_renderer::{
    Blend, Circle, Mesh, Projection, Sampler, Shader, Sprite, SpriteSheet, Texture, Viewpoint,
    Viewport,
};
pub use input::Input;
pub use inspector::{Inspector, InspectorLink, Placed};
#[doc(hidden)]
pub use jobs::{on_threads, start as start_jobs};
use layer::Stack;
pub use layer::{Commands, Context, Layer, Pick, Tick, Window};
use main_loop::run_headless;
pub use model::{Model, ModelNode, ModelRoot};
pub use name::Name;
pub use perf::PerfPanel;
pub use physics::{
    BodyType, BoxCollider2D, CircleCollider2D, Physics2D, RigidBody2D, step_physics,
};
use recording::Recorder;
#[doc(hidden)]
pub use recording::Replay;
pub use reflect::Reflect;
pub use script::{Script, run_scripts};
use snapshot::Snapshot;
pub use transform::Transform;

const BUILD: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

/// Describes an app to the engine: its name and its layers. An app's `main` is
/// `gg_engine::run(gg_engine::App::new("Sandbox").layer(Sandbox::new()))`.
pub struct App {
    name: &'static str,
    stack: Stack,
    custom_title_bar: bool,
    /// The panel that shows the game's view, in a dock that fills the main window.
    view_panel: Option<&'static str>,
    /// Where panels dock as they first show, in the order the app gave them.
    docked: Vec<(&'static str, Docked)>,
    /// What the world's IDs are drawn from, rather than a seed drawn at random.
    seed: Option<u64>,
    /// How far apart headless frames are, rather than a tick each.
    headless_step: Option<Duration>,
    /// What the files of the assets the layers add are named from.
    root: Option<PathBuf>,
    /// The project's manifest, as [`project`](Self::project) read it, or why it couldn't.
    project: Option<Result<Manifest>>,
}

impl App {
    /// `name` titles the window and names the log file.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            stack: Stack::default(),
            custom_title_bar: false,
            view_panel: None,
            docked: Vec::new(),
            seed: None,
            headless_step: None,
            root: None,
            project: None,
        }
    }

    /// Names each asset the layers add from a file by its path relative to `root`, the game's
    /// directory, as `env!("CARGO_MANIFEST_DIR")` gives it, with `/` between its parts, such as
    /// `assets/logo.png`: what scenes, snapshots, recordings and the tool API name a sprite's
    /// texture by, so that they find it again in another run, or on another machine. Without it,
    /// or for a file outside it, an asset is named by its path as the layer gave it. In a
    /// development build, each texture, model and font under it has an ID, kept in an ID file
    /// beside it, which a run that serves the tool API writes where it's missing, but a lockstep
    /// session's joiner, whose host writes them, as ARCHITECTURE.md's Project state describes.
    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// Makes the app the game of the project whose manifest, `project.ron`, is in `dir`, the
    /// game's directory, as `env!("CARGO_MANIFEST_DIR")` gives it, as ARCHITECTURE.md's Project
    /// state describes: names the assets the layers add from the project's asset root, as
    /// [`root`](Self::root) does; has [`Context::load_start_scene`] load its start scene; and has
    /// the tool API's session file name the manifest, which tools inside the project find the run
    /// by. A manifest that doesn't read ends the run as it starts, with its error, as does a build
    /// without `dev`, which reads none: shipping builds start from cooked data.
    #[must_use]
    pub fn project(mut self, dir: impl AsRef<Path>) -> Self {
        let manifest = Manifest::read(dir.as_ref());
        if let Ok(manifest) = &manifest {
            self.root = Some(manifest.root.clone());
        }
        self.project = Some(manifest);
        self
    }

    /// The project's manifest, as [`project`](Self::project) read it; `None` for an app that isn't
    /// a project's game, or one whose manifest didn't read.
    fn manifest(&self) -> Option<&Manifest> {
        self.project.as_ref()?.as_ref().ok()
    }

    /// Draws the world's IDs from `seed` rather than from a seed drawn at random, so that a run's
    /// IDs, and what follows from them, such as the order entities at one z draw in, are the same
    /// every run: for tests.
    #[doc(hidden)]
    #[must_use]
    pub fn seeded(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Has headless frames come `dt` apart, which their measures take them as, rather than a tick
    /// each, stepping the simulation as windowed runs do, so that they draw the world between its
    /// ticks: for tests. A `dt` less than a tick runs at most one a frame, however long one takes.
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

    /// Has the UI draw the frames of the app's windows instead of the OS, with the menus layers add
    /// through [`Ui::menu`] behind a button in the main window's title bar, and the tab bars of
    /// torn-off panels as their windows' title bars: for tools, which want the room. On macOS the
    /// OS keeps its buttons and edges. Games keep the OS's frame. The dock adds a View menu after
    /// the layers' menus, which opens and closes panels and saves, loads and resets the layout;
    /// gives tabs a menu that closes their panels; and puts buttons in the title bar that hide and
    /// show the panels left of, below and right of the game's view, as [`Ui::menu`] and
    /// [`Ui::panel`] say.
    #[must_use]
    pub fn custom_title_bar(mut self) -> Self {
        self.custom_title_bar = true;
        self
    }

    /// Shows the game's view in a panel of its own, `name`, which the user docks and floats as the
    /// layers' panels, and has the dock fill the main window, without the central area games keep:
    /// for tools. The view fills the dock until panels dock beside it, and it doesn't go into a
    /// window of its own. The layers get the mouse's presses and its wheel over the view, and the
    /// drags those start; and keys while none of egui's widgets has focus, no modal of egui's is
    /// open and the view shows, if the view has focus, which a press on it, or on what
    /// [`Ui::over_view`] shows over it, gives and a press anywhere else takes, or the cursor is
    /// over either while egui drags none of its own widgets; it has focus as the app starts.
    /// Modifier keys reach them wherever they're pressed while no widget of egui's has focus, and
    /// releases wherever they come.
    #[must_use]
    pub fn view_panel(mut self, name: &'static str) -> Self {
        self.view_panel = Some(name);
        self
    }

    /// Docks panel `panel` where `docked` says as it first shows in a layout that doesn't hold
    /// it: on the app's first run, after the View menu's Reset layout, and with a layout saved
    /// before the app had the panel. Beside the game's view, a panel takes a quarter of the view's
    /// panel, and beside another panel, half of it; tabbed with another, it leaves the tab that
    /// showed showing. Panels that first show in the same frame dock in the order the app gives
    /// them, whatever order the layers show them in. A target the layout doesn't hold yet stands
    /// for its own target, if the app gives it one, and so on: a panel that shows before its
    /// target docks beside the target's target, where it stays once the target shows. A panel
    /// whose target floats, or comes to none, floats, as the panels the app doesn't dock do.
    ///
    /// Three of the editor's panels dock around its view, the Inspector taking a quarter of the
    /// window, Entities a quarter of the rest, and Performance below the Inspector:
    ///
    /// ```
    /// use gg_engine::{App, Docked};
    ///
    /// let app = App::new("Editor")
    ///     .custom_title_bar()
    ///     .view_panel("Viewport")
    ///     .dock("Inspector", Docked::RightOf("Viewport"))
    ///     .dock("Entities", Docked::LeftOf("Viewport"))
    ///     .dock("Performance", Docked::Below("Inspector"));
    /// ```
    #[must_use]
    pub fn dock(mut self, panel: &'static str, docked: Docked) -> Self {
        self.docked.push((panel, docked));
        self
    }
}

/// Runs `app` until its window closes or `--frames` runs out, and returns the exit status; or,
/// with `--replay`, replays a recording of it.
///
/// It parses the flags every app shares, which `--help` lists, and starts logging. The status is
/// 0 on success, 1 on an error, a Vulkan validation message or a replay that goes another way,
/// and 2 on bad arguments.
pub fn run(app: App) -> ExitCode {
    let started = Instant::now();
    let mut argv = std::env::args_os();
    let program = argv
        .next()
        .and_then(|path| Some(Path::new(&path).file_stem()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| app.name.to_lowercase());
    run_args(app, started, &program, argv)
}

/// Runs `app` as [`run`] does, with `args` in place of the arguments the process was given after
/// its name: for tests, whose own are their harness's.
#[doc(hidden)]
pub fn run_with(app: App, args: impl IntoIterator<Item = impl Into<OsString>>) -> ExitCode {
    let program = app.name.to_lowercase();
    let args = args.into_iter().map(Into::into);
    run_args(app, Instant::now(), &program, args)
}

/// What [`run`] and [`run_with`] do with `args`, the arguments after the program's name, which
/// the usage names `program`, once the engine `started`.
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

    // An embedded run's tool reads its events through the tool API's `logs`, and its stderr too,
    // which would show each a second time.
    let log = gg_core::log::init(app.name, &beside_exe("logs"), !args.embedded);
    info!(
        version = env!("CARGO_PKG_VERSION"),
        app = app.name,
        build = BUILD,
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
        frames = args.frames,
        capture = args.capture.map(|frame| frame.get()),
        gpu = args.gpu.as_deref(),
        validation = args.validation,
        record = args.record.as_ref().map(|path| path.display().to_string()),
        replay = args.replay.as_ref().map(|path| path.display().to_string()),
        "Options"
    );

    if let Some(Err(error)) = &app.project {
        error!(code = error.code(), "{error:#}");
        return ExitCode::FAILURE;
    }
    // Before the replay too, which ticks on them as a run does.
    if let Err(error) = jobs::start(args.threads) {
        error!(code = error.code(), "{error:#}");
        return ExitCode::FAILURE;
    }
    // Before any renderer, which a replay doesn't need, and which a recording that can't be
    // written would keep from starting.
    if let Some(path) = &args.replay {
        return recording::replay(app, path, args.dump);
    }
    // Before the tool API starts, as tools take a run that serves it to have started, so that a
    // snapshot it can't take ends it first.
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
        if let Some(net) = &args.net {
            let net = net::Lockstep::start(net, app.name, app.stack.names())?;
            app.stack.lockstep(net);
        }
        // Before the renderer starts, so that tools find a run that hangs as it starts.
        let player = args.net.as_ref().map(|net| match net {
            args::Net::Host(_) => 0,
            args::Net::Join(_) => 1,
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
            // An embedded run's tool reaches it through the API alone, as its console is off, and
            // a stopped run plays only as the API's `play` asks: it ends, saying why, which the
            // tool hears of as the run ends, rather than running unseen or stopped for good.
            Err(error) if args.embedded || args.stopped => return Err(error),
            Err(error) => {
                let consequence = "tools can't reach this run";
                warn!(code = error.code(), "{error:#}; {consequence}");
                None
            }
        };
        if args.headless || args.embedded {
            run_headless(&mut app, &args, api, snapshot)
        } else {
            window::run(&mut app, &args, api, snapshot)
        }
    });
    match ran {
        Ok(ran) => ran.report(started, &args),
        Err(error) => {
            error!(code = error.code(), "{error:#}");
            ExitCode::FAILURE
        }
    }
}

/// What a run takes from its project's manifest, as [`App::project`] says: the manifest's path, the
/// project's asset root, its start scene's path and its scenes folders' paths, which hold the scene
/// files the tool API's `scene` opens and saves.
pub(crate) struct Manifest {
    pub(crate) path: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) start: PathBuf,
    #[cfg(feature = "dev")]
    pub(crate) scenes: Vec<PathBuf>,
}

impl Manifest {
    /// Reads the manifest of the project in directory `dir`.
    ///
    /// Fails as gg-api's `Project::read` does, with `project.read` or `project.bad`; and with
    /// `engine.no_project` in a build without `dev`, which reads none.
    pub(crate) fn read(dir: &Path) -> Result<Self> {
        #[cfg(feature = "dev")]
        {
            let project = gg_api::Project::read(dir).map_err(|failure| {
                // The codes it fails with, as the engine's codes are static.
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

/// The name of the font family [`egui_context`] binds to Inter's bold weight, which the
/// Inspector's axis buttons draw in.
pub(crate) const BOLD: &str = "Bold";

/// egui in the engine's UI font, Inter: its regular weight for proportional text, its digits and
/// minus sign tabular, all as wide as a figure space, so that numbers that change hold still; and
/// its bold in the family [`BOLD`] names, each followed by egui's own proportional fonts, for the
/// glyphs Inter lacks, such as egui's icons; monospace text stays in egui's font, and every size
/// egui's. Collapsing headers are framed, as wide as their `Ui`, in the light theme and the dark,
/// which follows the OS's. The fonts are in the binary, and egui parses a family's as it first lays
/// out text in it. Without egui's shortcut that closes the window, Ctrl or Cmd+Q: a game may use
/// those keys, and apps quit from their own menus.
fn egui_context() -> egui::Context {
    let egui = egui::Context::default();
    egui.options_mut(|options| options.quit_shortcuts.clear());
    let mut fonts = egui::FontDefinitions::default();
    let fallbacks = fonts.families[&egui::FontFamily::Proportional].clone();
    let inter: [(egui::FontFamily, &str, &'static [u8]); 2] = [
        // Inter's Regular, of version 4.001, but for its cmap, which maps 0 to 9 and U+2212 to the
        // glyphs its `tnum` feature substitutes for them, as Blender's Inter does: egui shapes
        // text without OpenType's optional features. Made with fontTools, by setting each of
        // those code points, in each cmap subtable, to the font's tnum lookups' substitute for
        // it, read from a copy of its own so that its GSUB saves unchanged; the head table's
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

/// How the engine's menus look: as egui's, but with items padded 8 points across and 4 down, as
/// roomy as Fluent's and JetBrains' menus' are rather than egui's 2 and 0, so that the light under
/// the pointer frames an item's text, rounded 4 points as theirs are. For each menu the engine
/// shows, and those its menus open, which take their parent's style.
pub(crate) fn menu_style(style: &mut egui::Style) {
    egui::menu::menu_style(style);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    let widgets = &mut style.visuals.widgets;
    for state in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        state.corner_radius = egui::CornerRadius::same(4);
    }
}

/// The directory `name` beside the executable, such as `target/debug/logs` for `cargo run`'s
/// logs, or in the working directory if the executable's path isn't known: where runs write their
/// logs, and development builds the tool API's session files and captures.
pub(crate) fn beside_exe(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UI draws in Inter: proportional text in its regular weight, its digits and minus sign
    /// as wide as a figure space, and the family [`BOLD`] names in its bold, each falling back to
    /// egui's own proportional fonts; monospace in egui's. Collapsing headers are framed in the
    /// light theme and the dark, whichever the OS has.
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
                // The axes' letters, and the ellipsis that ends menu items such as Save layout as…
                // and names cut short.
                let has = |c| characters.get(&c).is_some_and(in_font);
                assert!("XYZW…".chars().all(has), "{font} lacks one");
                let id = egui::FontId::new(14.0, family);
                assert!(fonts.has_glyphs(&id, &icon), "{font} falls back");
            }
            // Tabular: a number's digits, its sign or the figure space that holds its place, and
            // its kerning, which Inter's proportional digits have, don't change its width.
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
