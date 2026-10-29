//! What apps build on: the main loop, which steps the simulation at a fixed rate whatever the frame
//! rate, window and input, and the actions that name what a game does with it, and layers, which
//! change the world at its ticks, and their UI; the world of entities and their components, which
//! it draws between its ticks, handing each frame's render data to `gg-renderer` on a thread of its
//! own, a frame behind, and the job system, which a tick's systems split a query's rows across;
//! reflection, which describes the components' types so that tools reach them by name, and the
//! Inspector, which edits them; scripts, components that act at ticks; scenes and prefabs, the
//! world's entities as canonical text; operations, the authoring changes made to the world, each
//! with its inverse, and their log, which undoes and redoes them; recordings of runs, the commands
//! their ticks applied, which replay without a GPU to the same world, naming where one goes another
//! way; and measuring each frame, for the log and the Performance panel; loading what layers add,
//! images as textures and glTF models, whose nodes spawn as entities; the pipeline cache's file,
//! which windowed runs keep; and in development builds, behind `dev`, compiling games' shaders
//! again as their files change. A library of assets to come.
//!
//! An app's `main` starts the engine; there is no separate entry-point crate.

// So that the derives' code, which names `::gg_engine`, works here too.
extern crate self as gg_engine;

mod actions;
mod args;
mod camera;
mod chrome;
// In development builds alone, as `dev` has it.
#[cfg(feature = "dev")]
mod dialog;
mod dirs;
mod dock;
mod draw;
mod ecs;
mod event;
mod gltf;
// In development builds alone, as `dev` has it.
#[cfg(feature = "dev")]
mod hot_reload;
mod image;
mod input;
mod inspector;
mod interpolation;
mod jobs;
mod layer;
mod model;
mod name;
mod pads;
mod perf;
mod physics;
mod pipeline_cache;
mod recording;
pub mod reflect;
mod render_thread;
mod script;
mod transform;
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
use std::mem;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gg_core::Result;
use gg_renderer::{MAX_TEXTURE_SIZE, Renderer, Settings, Validation};
use tracing::{debug, error, info, warn};

pub use actions::{Actions, Binding, Change, Source, Toward};
use args::Args;
pub use camera::{
    Camera, CameraAction, FlyAction, FlyCameraController, OrthographicCameraController,
};
use chrome::Chrome;
#[cfg(feature = "dev")]
pub use dialog::FileDialog;
use dock::Dock;
pub use dock::{Docked, Ui};
pub use ecs::{
    Bundle, Component, Difference, Entities, Entity, EntityId, Log, Operation, Parent, Pose, Query,
    QueryIter, QueryIterMut, ReadOnlyQuery, Resource, World, WorldHash, Writable, read_scene,
    write_scene,
};
/// The UI library layers draw with, in [`Layer::ui`]; apps use it from here, so that they
/// share the engine's version.
pub use egui;
pub use event::{Event, Key, MouseButton, Pad, PadButton, Scroll, Stick, Trigger};
/// The engine's vectors, matrices and quaternions; apps use them from here, so that they share
/// the engine's version.
pub use gg_core::math;
/// The engine's profiling: zones, plots and frame marks for Tracy, which compile to nothing unless
/// the `tracy` feature is on. Apps use them from here, such as `let _zone = gg_engine::zone!();`.
pub use gg_core::{plot, profile, zone};
/// A mesh to draw with a shader and its parameters, without their types, so that an entity can
/// have one, which the engine draws as it draws the world.
pub use gg_renderer::MeshDraw;
/// What layers draw, which they add and draw through their [`Context`]: sprites, and the sprite
/// sheets whose cells they show; circles; lines; meshes; shaders
/// for their vertex types, each added with how it blends; the textures those shaders and sprites
/// sample, and the samplers they sample them with; the viewpoint the game's view sees them from,
/// and its projection, which a [`Camera`] has too; and where that view is in the window.
pub use gg_renderer::{
    Blend, Circle, Line, Mesh, Projection, Sampler, Shader, Sprite, SpriteSheet, Texture,
    Viewpoint, Viewport,
};
pub use input::Input;
pub use inspector::{Inspector, InspectorLink};
#[doc(hidden)]
pub use jobs::{on_threads, start as start_jobs};
use layer::Stack;
pub use layer::{Commands, Context, Layer, Pick, Tick, Window};
pub use model::Model;
pub use name::Name;
pub use perf::PerfPanel;
use perf::{Perf, Summary};
pub use physics::{
    BodyType, BoxCollider2D, CircleCollider2D, Physics2D, RigidBody2D, step_physics,
};
use recording::Recorder;
#[doc(hidden)]
pub use recording::Replay;
pub use reflect::Reflect;
use render_thread::{Render, RenderThread, TextureChanges};
pub use script::{Script, run_scripts};
pub use transform::Transform;
use viewports::Drawn;

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
        }
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

    /// Has the UI draw the frames of the app's windows instead of the OS, with the menus layers
    /// add through [`Ui::menu`] in the main window's title bar, and the tab bars of torn-off
    /// panels as their windows' title bars: for tools, which want the room. On macOS the OS keeps
    /// its buttons and edges. Games keep the OS's frame. The dock adds a View menu after the
    /// layers' menus, which shows and hides panels and saves, loads and resets the layout, and
    /// gives tabs a menu that closes their panels, as [`Ui::menu`] and [`Ui::panel`] say.
    #[must_use]
    pub fn custom_title_bar(mut self) -> Self {
        self.custom_title_bar = true;
        self
    }

    /// Shows the game's view in a panel of its own, `name`, which the user docks and floats as the
    /// layers' panels, and has the dock fill the main window, without the central area games
    /// keep: for tools. The view fills the dock until the user places other panels beside it, and
    /// it doesn't go into a window of its own. The layers get the mouse's presses and its wheel
    /// over the view, and the drags those start; and keys while none of egui's widgets has focus,
    /// no modal of egui's is open and the view shows, if the view has focus, which a press on it,
    /// or on what [`Ui::over_view`] shows over it, gives and a press anywhere else takes, or the
    /// cursor is over either; it has focus as the app starts. Modifier keys reach them wherever
    /// they're pressed while no widget of egui's has focus, and releases wherever they come.
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
    /// target docks beside the target's target for now. A panel whose target floats, or comes to
    /// none, floats, as the panels the app doesn't dock do.
    ///
    /// The editor's panels dock around its view, the Inspector taking a quarter of the window,
    /// Entities a quarter of the rest, and Performance below the Inspector:
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

    let log = gg_core::log::init(app.name, &log_dir());
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
    if let Some(path) = log {
        info!(path = %path.display(), "Writing the JSON log");
    }
    debug!(
        headless = args.headless,
        frames = args.frames,
        capture = args.capture.map(|frame| frame.get()),
        gpu = args.gpu.as_deref(),
        validation = args.validation,
        record = args.record.as_ref().map(|path| path.display().to_string()),
        replay = args.replay.as_ref().map(|path| path.display().to_string()),
        "Options"
    );

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
    let recorder = args
        .record
        .as_deref()
        .map(|path| Recorder::create(path, app.name));
    let ran = recorder.transpose().and_then(|recorder| {
        if let Some(recorder) = recorder {
            app.stack.record(recorder);
        }
        if args.headless {
            run_headless(&mut app, &args)
        } else {
            window::run(&mut app, &args)
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

/// Renders offscreen, without a window, as fast as the GPU allows, with a tick of the simulation
/// each frame, so that a run of a set number of frames runs as many ticks on any machine. Frames
/// render on the render thread, as a windowed run's do, the last as the run ends.
fn run_headless(app: &mut App, args: &Args) -> Result<Ran> {
    let (width, height) = (1280, 720);
    let renderer = Renderer::headless(&settings(app, args), width, height)?;
    let mut render = RenderThread::start(renderer)?;
    let mut cx = Context::of_run(None, app.seed);
    let mut clock = Clock::default();
    let mut stepper = match app.headless_step {
        Some(_) => Stepper::default(),
        None => Stepper::lockstep(),
    };
    let egui = egui_context();
    // Headless runs neither read nor save the layout, so that they're reproducible, and have no
    // window to frame.
    let mut dock = Dock::new(
        None,
        Chrome::Os,
        app.view_panel,
        app.custom_title_bar,
        &app.docked,
    );
    let mut egui_runs = true;
    // The frames handed over to render, and those rendered.
    let (mut handed, mut frames) = (0, 0);
    app.stack.attach(&mut cx);
    let rendered = (|| -> Result<()> {
        while args.frames.is_none_or(|limit| handed < limit) {
            // One point is one pixel, as on a display without scaling.
            let input = egui_runs.then(|| egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                max_texture_side: Some(MAX_TEXTURE_SIDE),
                time: Some(clock.elapsed().as_secs_f64()),
                ..Default::default()
            });
            let dt = app.headless_step.unwrap_or(clock.start_frame());
            let pass = run_frame(
                &mut app.stack,
                &mut cx,
                dt,
                &mut stepper,
                input.map(|input| (&egui, &mut dock, input)),
            );
            let (ui, tessellated, view) = match pass {
                Some((mut output, layers_draw)) => {
                    egui_runs = keeps_egui(&app.stack, layers_draw, Chrome::Os, &dock);
                    let (ui, tessellated) = tessellate(&egui, &mut output);
                    let view = viewport(dock.view(), ui.pixels_per_point);
                    (ui, tessellated, view)
                }
                None => {
                    let view = Viewport {
                        origin: [0, 0],
                        size: [width, height],
                    };
                    (MainUi::none(), Duration::ZERO, view)
                }
            };
            if take_back(&mut render, &mut cx, tessellated)?.is_some() {
                frames += 1;
            }
            hand_off(&mut render, &mut cx, view, ui, Vec::new());
            handed += 1;
            app.stack.recorded()?;
        }
        Ok(())
    })();
    // The last frame ends here, and the frame handed over last renders before the run's end is
    // timed.
    cx.perf.finish(clock.since_last(Instant::now()));
    let (renderer, last) = render.stop();
    let rendered = rendered.and_then(|()| {
        if let Some(last) = last {
            last.presented?;
            frames += 1;
        }
        Ok(())
    });
    let stopping = Instant::now();
    let _zone = gg_core::zone!("shutdown");
    app.stack.detach(&mut cx);
    rendered?;
    app.stack.recorded()?;
    let perf = mem::take(&mut cx.perf);
    Ok(Ran::finish(
        renderer, frames, &clock, &stepper, perf, stopping,
    ))
}

/// The most pixels a side of egui's textures may have, the most every device the engine runs on
/// takes, which each pass's input tells egui: every window's alike, since egui makes its font atlas
/// again for a pass whose limit isn't the last one's.
const MAX_TEXTURE_SIDE: usize = MAX_TEXTURE_SIZE as usize;

/// The name of the font family [`egui_context`] binds to Inter's bold weight, which the
/// Inspector's axis buttons draw in.
pub(crate) const BOLD: &str = "Bold";

/// egui in the engine's UI font, Inter: its regular weight for proportional text, its digits and
/// minus sign tabular, all as wide as a figure space, so that numbers that change hold still; and
/// its bold in the family [`BOLD`] names, each followed by egui's own proportional fonts, for the
/// glyphs Inter lacks, such as egui's icons; monospace text stays in egui's font, and every size
/// egui's.
/// Collapsing headers are framed, as wide as their `Ui`, in the light theme and the dark, which
/// follows the OS's. The fonts are in the binary, and egui parses a family's as it first lays out
/// text in it. Without egui's shortcut that closes the window, Ctrl or Cmd+Q: a game may use
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
        (
            egui::FontFamily::Proportional,
            "Inter",
            include_bytes!("../assets/fonts/Inter/Inter-Regular.ttf"),
        ),
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

/// The first half of a turn of the main loop: starts the frame, `dt` after the last, for Tracy and
/// the frame's measure, runs the ticks of the simulation that the time owes, as `stepper` steps
/// it, copying where the world's entities are as each starts, updates the layers, draws the world
/// after what they drew, as far on from where the last tick found it to where it left it as the
/// frame is past the tick, and, given egui, the dock and egui's input in `ui`, has the layers draw
/// their UI, in the dock and with egui. Returns that pass's output, and whether any layer draws
/// UI, as [`Stack::ui`] says. [`tessellate`] starts the second half, which hands what egui made
/// of it to the render thread.
///
/// egui allocates as it lays out here, and as [`tessellate`] tessellates, so frames that run its
/// pass allocate; the UI is a development overlay, and an app that draws none stops running it,
/// as [`keeps_egui`] says.
fn run_frame(
    stack: &mut Stack,
    cx: &mut Context,
    dt: Duration,
    stepper: &mut Stepper,
    ui: Option<(&egui::Context, &mut Dock, egui::RawInput)>,
) -> Option<(egui::FullOutput, bool)> {
    gg_core::profile::frame_mark();
    cx.perf.start_frame(dt);
    let _zone = gg_core::zone!();
    let start = Instant::now();
    let tick = |tick| {
        cx.capture();
        stack.tick(cx, tick);
    };
    stepper.step(dt, tick, || start.elapsed());
    let ticked = Instant::now();
    stack.update(cx, dt);
    let alpha = stepper.alpha();
    gg_core::plot!("alpha", f64::from(alpha));
    cx.draw_world(alpha);
    let updated = Instant::now();
    let pass = ui.map(|(egui, dock, input)| {
        let _zone = gg_core::zone!("egui");
        let mut layers_draw = false;
        // egui may run the pass again in a frame, as it asks for one.
        let output = egui.run_ui(input, |root| {
            dock.begin(root);
            layers_draw |= stack.ui(cx, &mut Ui::new(root, dock));
            dock.end(root);
        });
        (output, layers_draw)
    });
    cx.perf
        .ran(ticked - start, updated - ticked, updated.elapsed());
    pass
}

/// Whether egui runs from the next frame on, after a pass in which `layers_draw` says whether any
/// of `stack`'s layers draw UI, in windows framed as `chrome` says. Once none does, the OS frames
/// the windows, and the game's view isn't in `dock`'s panel, egui would show nothing, so it stops
/// for the rest of the run: whether a layer draws UI is its type's, which no later frame changes.
fn keeps_egui(stack: &Stack, layers_draw: bool, chrome: Chrome, dock: &Dock) -> bool {
    let keeps = layers_draw || chrome != Chrome::Os || dock.is_dockspace();
    if !keeps {
        debug!(
            layers = ?stack.names(),
            "No layer draws UI, so frames run without egui from the next"
        );
    }
    keeps
}

/// What the main window's pass drew in a frame, tessellated, for the render thread.
struct MainUi {
    primitives: Vec<egui::ClippedPrimitive>,
    textures: TextureChanges,
    pixels_per_point: f32,
}

impl MainUi {
    /// Nothing, from a frame that ran no pass.
    fn none() -> Self {
        Self {
            primitives: Vec::new(),
            textures: TextureChanges::default(),
            pixels_per_point: 1.0,
        }
    }
}

/// The second half of a turn of the main loop starts here, while the frame before renders:
/// tessellates the UI the main window's pass drew, taking it and its texture changes from `output`,
/// and returns it, and how long that took.
fn tessellate(egui: &egui::Context, output: &mut egui::FullOutput) -> (MainUi, Duration) {
    let _zone = gg_core::zone!("tessellate UI");
    let start = Instant::now();
    let textures = TextureChanges(mem::take(&mut output.textures_delta));
    let primitives = egui.tessellate(mem::take(&mut output.shapes), output.pixels_per_point);
    let ui = MainUi {
        primitives,
        textures,
        pixels_per_point: output.pixels_per_point,
    };
    (ui, start.elapsed())
}

/// Takes the renderer back from rendering the frame before, which this frame, having tessellated
/// its own UI in `tessellated`, is measured with, and the answers to picks that came in as it
/// rendered, which `cx` gives in place of the last; and returns whether that frame presented:
/// `None` if no frame was rendering.
fn take_back<R: Render>(
    render: &mut RenderThread<R>,
    cx: &mut Context,
    tessellated: Duration,
) -> Result<Option<bool>> {
    let start = Instant::now();
    let rendered = render.take();
    cx.perf
        .rendered(tessellated, start.elapsed(), rendered.as_ref());
    let answers = if rendered.is_some() {
        &render.frame().picked[..]
    } else {
        &[]
    };
    cx.take_answers(answers);
    rendered.map(|rendered| rendered.presented).transpose()
}

/// Hands the frame over to the render thread: what the layers drew in the game's view, `view` in
/// the main window's pixels, the UI in `ui` over it, and what the windows of their own drew, in
/// `windows`.
fn hand_off<R: Render>(
    render: &mut RenderThread<R>,
    cx: &mut Context,
    view: Viewport,
    ui: MainUi,
    windows: Vec<Drawn>,
) {
    render.hand(|frame| {
        cx.extract(frame, view);
        frame.primitives = ui.primitives;
        frame.textures = ui.textures;
        frame.pixels_per_point = ui.pixels_per_point;
        frame.windows = windows;
    });
}

/// The pixels of `rect`, in points of `ppp` pixels each, rounded as egui rounds its clip
/// rectangles.
fn viewport(rect: egui::Rect, ppp: f32) -> Viewport {
    let min = (rect.min.to_vec2() * ppp).round();
    let max = (rect.max.to_vec2() * ppp).round();
    // Casts saturate, so a rect without an area has none in pixels either.
    Viewport {
        origin: [min.x as u32, min.y as u32],
        size: [(max.x - min.x) as u32, (max.y - min.y) as u32],
    }
}

/// Measures frames: the time between them, and since the first.
#[derive(Default)]
struct Clock {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Clock {
    /// Starts a frame, and returns the time since the last one started; zero for the first, and
    /// for the first after [`Clock::resume`].
    fn start_frame(&mut self) -> Duration {
        let now = Instant::now();
        let dt = self.last.map_or(Duration::ZERO, |last| now - last);
        self.first.get_or_insert(now);
        self.last = Some(now);
        dt
    }

    /// Has the next frame count from its own start, as the first does, across a pause in which no
    /// frames run, such as while the window is minimized, as the pause starts and as it ends:
    /// otherwise the layers would take the pause for one long frame, and so would the frame times.
    fn resume(&mut self) {
        self.last = None;
    }

    fn elapsed(&self) -> Duration {
        self.first.map_or(Duration::ZERO, |first| first.elapsed())
    }

    /// The time from the last frame's start to `at`; zero before the first frame.
    fn since_last(&self, at: Instant) -> Duration {
        self.last
            .map_or(Duration::ZERO, |last| at.saturating_duration_since(last))
    }
}

/// The most ticks a frame runs: 125 ms of the simulation. Past them, the simulation falls behind
/// real time, rather than taking the frame rate down with it.
const MOST_TICKS: u64 = 8;

/// Turns frames' time into the simulation's ticks, as many as the time owed holds, so that the
/// simulation steps at its fixed rate whatever the frame rate.
#[derive(Default)]
struct Stepper {
    /// The time not yet simulated: less than a tick, between frames.
    owed: Duration,
    /// The ticks run, which number the next.
    ticks: u64,
    /// The whole ticks the time owed held that no frame ran.
    dropped: u64,
    /// One tick every frame, whatever its time.
    lockstep: bool,
}

impl Stepper {
    /// A tick every frame, as headless runs step, so that a run of a set number of frames runs as
    /// many ticks, however fast its frames are.
    fn lockstep() -> Self {
        Self {
            lockstep: true,
            ..Self::default()
        }
    }

    /// Runs, through `tick`, the ticks owed by a frame `dt` after the last, numbered on from the
    /// last: one for each whole tick the time owed holds, at most [`MOST_TICKS`], and no more once
    /// `took`, the time the frame's ticks have taken so far, is longer than the time they've
    /// simulated, so that a simulation slower than real time slows down rather than spending every
    /// frame catching up. The whole ticks not run are dropped, and the fraction of one left over
    /// carries to the next frame. In lockstep, exactly one. Returns how many ran.
    fn step(
        &mut self,
        dt: Duration,
        mut tick: impl FnMut(Tick),
        took: impl Fn() -> Duration,
    ) -> u64 {
        let due = if self.lockstep {
            1
        } else {
            self.owed += dt;
            let step = Tick::DT.as_nanos();
            let owed = self.owed.as_nanos();
            self.owed = Duration::from_nanos((owed % step) as u64);
            (owed / step) as u64
        };
        let mut ran = 0;
        while ran < due.min(MOST_TICKS) {
            tick(Tick { number: self.ticks });
            self.ticks += 1;
            ran += 1;
            // Slower than real time: the ticks left would only fall further behind.
            if took() > Tick::DT * ran as u32 {
                break;
            }
        }
        self.dropped += due - ran;
        ran
    }

    /// How far the frame is past its last tick, as a fraction of a tick, from 0 to less than 1,
    /// which the world draws that far on from where the tick found it to where it left it; 1 in
    /// lockstep, so that a headless frame draws its tick as it leaves the world.
    fn alpha(&self) -> f32 {
        if self.lockstep {
            return 1.0;
        }
        (self.owed.as_nanos() as f64 / Tick::DT.as_nanos() as f64) as f32
    }
}

fn settings<'a>(app: &'a App, args: &'a Args) -> Settings<'a> {
    Settings {
        app_name: app.name,
        validation: args.validation,
        gpu: args.gpu.as_deref(),
        vsync: args.vsync,
        uploads: None,
        pipeline_cache: None,
        capture: args.capture,
    }
}

/// Logs go in `logs` beside the executable, such as `target/debug/logs` for `cargo run`.
fn log_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("logs")))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

/// What a run did, reported when it ends.
struct Ran {
    frames: u64,
    /// The simulation's ticks.
    ticks: u64,
    /// The whole ticks the frames' time held that they didn't run, which the simulation fell
    /// behind by.
    ticks_dropped: u64,
    validation: Validation,
    /// The GPU it rendered on.
    gpu: String,
    /// When its first frame started, if one did.
    first_frame_at: Option<Instant>,
    shutdown: Duration,
    perf: Summary,
}

impl Ran {
    /// Shuts the renderer down first, so that what validation reports on the way, such as
    /// objects left undestroyed, counts too. `stopping` is when the run began to end, after its
    /// last frame rendered, `stepper` what stepped its simulation, and `perf` what it measured of
    /// its frames, the last of them finished.
    fn finish(
        renderer: Renderer,
        frames: u64,
        clock: &Clock,
        stepper: &Stepper,
        perf: Perf,
        stopping: Instant,
    ) -> Self {
        let validation = renderer.validation();
        let gpu = renderer.gpu_name().to_owned();
        drop(renderer);
        Self {
            frames,
            ticks: stepper.ticks,
            ticks_dropped: stepper.dropped,
            validation,
            gpu,
            first_frame_at: clock.first,
            shutdown: stopping.elapsed(),
            perf: perf.summary(),
        }
    }

    /// Logs how long the run's frames took, then how it ended: `engine.perf`, whose times are in
    /// milliseconds, comes first, so that a run that fails validation has it too. `started` is
    /// when the engine started, and `args` how it ran.
    fn report(self, started: Instant, args: &Args) -> ExitCode {
        let perf = &self.perf;
        info!(
            code = "engine.perf",
            frames = self.frames,
            ticks = self.ticks,
            ticks_dropped = self.ticks_dropped,
            measured = perf.measured,
            percentile_frames = perf.percentile_frames,
            startup_ms = self
                .first_frame_at
                .map(|first| perf::millis(first.saturating_duration_since(started))),
            first_frame_ms = perf.first_frame,
            shutdown_ms = perf::millis(self.shutdown),
            frame_ms_p50 = perf.frame.p50,
            frame_ms_p95 = perf.frame.p95,
            frame_ms_p99 = perf.frame.p99,
            frame_ms_mean = perf.frame.mean,
            frame_ms_max = perf.frame.max,
            ticks_ms_mean = perf.ticks.mean,
            ticks_ms_p95 = perf.ticks.p95,
            update_ms_mean = perf.update.mean,
            update_ms_p95 = perf.update.p95,
            ui_ms_mean = perf.ui.mean,
            ui_ms_p95 = perf.ui.p95,
            handoff_ms_mean = perf.handoff.mean,
            handoff_ms_p95 = perf.handoff.p95,
            render_ms_mean = perf.render.mean,
            render_ms_p95 = perf.render.p95,
            wait_ms_mean = perf.wait.mean,
            wait_ms_p95 = perf.wait.p95,
            draws_max = perf.draws_max,
            instances_max = perf.instances_max,
            triangles_max = perf.triangles_max,
            gpu = self.gpu.as_str(),
            build = BUILD,
            headless = args.headless,
            vsync = args.vsync,
            validated = self.validation.enabled(),
            "Frame times"
        );
        // A frame past `--frames` is refused, but a window closed early ends a run before it.
        if let Some(frame) = args.capture
            && self.frames < frame.get()
        {
            warn!(
                code = "engine.not_captured",
                frame = frame.get(),
                frames = self.frames,
                "Frame {frame} wasn't captured: the run ended after {} frames",
                self.frames
            );
        }
        let messages = self.validation.messages();
        if messages > 0 {
            error!(
                code = "engine.validation",
                messages,
                "Vulkan validation reported {messages} problem(s); the warnings and errors above say what"
            );
            return ExitCode::FAILURE;
        }
        info!(
            frames = self.frames,
            validated = self.validation.enabled(),
            "Exited cleanly"
        );
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::f32::consts::FRAC_PI_2;
    use std::iter;
    use std::ops::ControlFlow;
    use std::rc::Rc;

    use gg_core::math::{Mat4, Quat, Vec3, Vec4};
    use gg_renderer::Draws;

    use super::*;

    /// Spawns a red sprite as it attaches, and draws a sprite of its own each update.
    struct Drawer;

    fn red() -> Sprite {
        Sprite {
            color: Vec4::new(1.0, 0.0, 0.0, 1.0),
            ..Sprite::default()
        }
    }

    impl Layer for Drawer {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn((Transform::IDENTITY, red()));
        }

        fn update(&mut self, cx: &mut Context, _: Duration) {
            cx.draw_sprite(Mat4::IDENTITY, Sprite::default());
        }
    }

    /// Spawns two sprites as it attaches: the slider, at x 0, which moves 1 in x at each tick, and
    /// the jumper, at x 10, which jumps 90 at tick 3, resetting its interpolation; and spawns one
    /// at x 5 at tick 2. Each has a z of its own, so that they draw in that order.
    #[derive(Default)]
    struct Slider {
        slider: Option<Entity>,
        jumper: Option<Entity>,
    }

    /// Where [`Slider`]'s sprites are: at `x`, and at their own `z`.
    fn slid(x: f32, z: f32) -> Transform {
        Transform {
            translation: Vec3::new(x, 0.0, z),
            ..Transform::IDENTITY
        }
    }

    impl Layer for Slider {
        fn attach(&mut self, cx: &mut Context) {
            let world = cx.world_mut();
            self.slider = Some(world.spawn((slid(0.0, 0.0), red())));
            self.jumper = Some(world.spawn((slid(10.0, 0.1), red())));
        }

        fn tick(&mut self, world: &mut World, tick: Tick) {
            let [slider, jumper] = [self.slider, self.jumper].map(|it| it.expect("attached"));
            world
                .get_mut::<Transform>(slider)
                .expect("alive")
                .translation
                .x += 1.0;
            match tick.number {
                2 => {
                    world.spawn((slid(5.0, 0.2), red()));
                }
                3 => {
                    world
                        .get_mut::<Transform>(jumper)
                        .expect("alive")
                        .translation
                        .x += 90.0;
                    world.reset_interpolation(jumper).expect("alive");
                }
                _ => {}
            }
        }
    }

    /// Runs a frame `dt` after the last, of a 640 by 360 view, and returns what it drew.
    fn drawn(
        stack: &mut Stack,
        cx: &mut Context,
        dt: Duration,
        stepper: &mut Stepper,
        (egui, dock): (&egui::Context, &mut Dock),
    ) -> Draws {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            ..Default::default()
        };
        let pass = run_frame(stack, cx, dt, stepper, Some((egui, dock, input)));
        let (mut output, _) = pass.expect("a pass, given egui");
        // As the renderer would have.
        output.textures_delta.clear();
        let mut frame = render_thread::Extracted::default();
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        cx.extract(&mut frame, viewport);
        frame.draws
    }

    /// What [`Slider`]'s sprites in `world` draw as: the slider at `slider`, the jumper at
    /// `jumper`, and the one spawned at tick 2, once it's spawned, each with its entity's ID,
    /// found by its z.
    fn slider_draws(world: &World, slider: f32, jumper: f32, spawned: bool) -> Draws {
        let id = |z: f32| {
            let mut placed = world.query::<(Entity, &Transform)>();
            let found = placed.find(|(_, at)| at.translation.z == z);
            let (entity, _) = found.expect("a sprite at each z");
            world.id(entity).expect("alive").get()
        };
        let mut want = Draws::default();
        let mut sprite = |x: f32, z: f32| {
            want.extend_sprites(iter::once((slid(x, z).matrix(), red(), id(z))));
        };
        sprite(slider, 0.0);
        sprite(jumper, 0.1);
        if spawned {
            sprite(5.0, 0.2);
        }
        want
    }

    /// Spawns an entity as it attaches, which it turns a quarter turn a second at each tick, and
    /// notes each tick it gets.
    struct Turner(Rc<RefCell<Vec<Tick>>>);

    impl Layer for Turner {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Transform::IDENTITY);
        }

        fn tick(&mut self, world: &mut World, tick: Tick) {
            let turn = Quat::from_rotation_z(FRAC_PI_2 * Tick::DT.as_secs_f32());
            for transform in world.query_mut::<&mut Transform>() {
                transform.rotation = (turn * transform.rotation).normalize();
            }
            self.0.borrow_mut().push(tick);
        }
    }

    /// A command of [`Queuer`]'s: spawns an entity turned by this angle.
    #[derive(Reflect)]
    struct Spawn(f32);

    /// Queues a spawn as it attaches, and at every third update, which its next tick makes.
    #[derive(Default)]
    struct Queuer {
        commands: Vec<Spawn>,
        updates: u32,
    }

    impl Layer for Queuer {
        fn attach(&mut self, _: &mut Context) {
            self.commands.push(Spawn(0.5));
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Spawn(angle) in self.commands.drain(..) {
                world.spawn(Transform {
                    rotation: Quat::from_rotation_z(angle),
                    ..Transform::IDENTITY
                });
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.commands)
        }

        fn update(&mut self, _: &mut Context, _: Duration) {
            self.updates += 1;
            if self.updates.is_multiple_of(3) {
                self.commands.push(Spawn(self.updates as f32 * 0.25));
            }
        }
    }

    /// A command of [`Placer`]'s: spawns this sprite, whose texture is opaque, which no value
    /// makes.
    #[derive(Reflect)]
    struct Place(Sprite);

    /// Queues a sprite to place as it attaches, which its next tick spawns.
    #[derive(Default)]
    struct Placer(Vec<Place>);

    impl Layer for Placer {
        fn attach(&mut self, _: &mut Context) {
            self.0.push(Place(red()));
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Place(sprite) in self.0.drain(..) {
                world.spawn((Transform::IDENTITY, sprite));
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.0)
        }
    }

    /// Notes its ticks and its updates, in the order they come.
    struct Order(Rc<RefCell<Vec<String>>>);

    impl Layer for Order {
        fn tick(&mut self, _: &mut World, tick: Tick) {
            self.0.borrow_mut().push(format!("tick {}", tick.number));
        }

        fn update(&mut self, _: &mut Context, _: Duration) {
            self.0.borrow_mut().push("update".to_owned());
        }
    }

    /// What [`Pilot`]'s keys do.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Act {
        Push,
        Tap,
    }

    /// A command of [`Pilot`]'s: a change of one of its actions.
    #[derive(Reflect)]
    enum Control {
        Push(f32),
        Tap,
    }

    /// What [`Pilot`]'s commands leave: how it's pushed, how many ticks it was pushed at, and its
    /// taps.
    #[derive(Clone, Copy, Component)]
    struct Piloted {
        push: f32,
        pushed: u32,
        taps: u32,
    }

    /// Pushes its entity while the left or the right arrow is held, and counts Space's taps,
    /// queuing its actions' changes as its events come, and noting the tick each applies at.
    struct Pilot {
        actions: Actions<Act>,
        commands: Vec<Control>,
        applied: Rc<RefCell<Vec<u64>>>,
    }

    impl Pilot {
        fn new(applied: Rc<RefCell<Vec<u64>>>) -> Self {
            let mut actions = Actions::default();
            let push = Binding::Axis {
                negative: Source::Key(Key::ArrowLeft),
                positive: Source::Key(Key::ArrowRight),
            };
            let tap = Binding::Button(Source::Key(Key::Space));
            let bound = actions.bind(Act::Push, push);
            bound
                .and_then(|()| actions.bind(Act::Tap, tap))
                .expect("bound");
            Self {
                actions,
                commands: Vec::new(),
                applied,
            }
        }
    }

    impl Layer for Pilot {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Piloted {
                push: 0.0,
                pushed: 0,
                taps: 0,
            });
        }

        fn tick(&mut self, world: &mut World, tick: Tick) {
            let mut piloted = world.query_mut::<&mut Piloted>();
            let piloted = piloted.next().expect("the pilot's entity");
            for command in self.commands.drain(..) {
                match command {
                    Control::Push(by) => piloted.push = by,
                    Control::Tap => piloted.taps += 1,
                }
                self.applied.borrow_mut().push(tick.number);
            }
            if piloted.push != 0.0 {
                piloted.pushed += 1;
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.commands)
        }

        fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
            let Self {
                actions, commands, ..
            } = self;
            actions.event(event, |change| match change {
                Change::Axis(Act::Push, by) => commands.push(Control::Push(by)),
                Change::Pressed(Act::Tap) => commands.push(Control::Tap),
                _ => {}
            });
            ControlFlow::Continue(())
        }
    }

    #[test]
    fn the_simulation_steps_at_a_fixed_rate_whatever_the_frame_rate() {
        let ms = Duration::from_millis;
        // Each a second of frames.
        let runs = [
            vec![ms(1); 1000],
            vec![ms(10); 100],
            vec![Tick::DT; 64],
            vec![ms(40); 25],
            [ms(2), ms(48)].repeat(20),
        ];
        let mut first = None;
        for frames in runs {
            let ticks = Rc::new(RefCell::new(Vec::new()));
            let mut stack = Stack::default();
            stack.push_layer(Turner(ticks.clone()));
            let mut cx = Context::new(None);
            stack.attach(&mut cx);
            let mut stepper = Stepper::default();
            for dt in frames {
                // A frame without time, as the first is and the first after a pause, runs no tick.
                for dt in [Duration::ZERO, dt] {
                    stepper.step(dt, |tick| stack.tick(&mut cx, tick), || Duration::ZERO);
                }
            }
            let ticks = ticks.borrow();
            let numbers: Vec<_> = ticks.iter().map(|tick| tick.number).collect();
            assert_eq!(numbers, (0..64).collect::<Vec<_>>());
            let last = ticks.last().map(|tick| tick.time());
            assert_eq!(last, Some(Duration::from_secs(1)));
            let turned = cx.world().query::<&Transform>().next();
            let turned = turned.map(|transform| transform.rotation.to_array().map(f32::to_bits));
            assert_eq!(turned, *first.get_or_insert(turned));
        }
    }

    /// An app of a layer whose commands its updates queue, under one without.
    fn two_layers() -> App {
        let app = App::new("Recorded").layer(Queuer::default());
        app.layer(Turner(Rc::default()))
    }

    /// A run of `app` recorded as it goes, frames `frames` apart, as a windowed run steps them:
    /// its recording, the ticks it ran, and its world's hash as it ended.
    fn recorded(app: App, frames: &[Duration]) -> (String, u64, WorldHash) {
        let (text, ticks, world) = recorded_with(app, frames, |_, _, _| {});
        (text, ticks, world.hash())
    }

    /// As [`recorded`], with `between` sending the stack events before each frame, given the ticks
    /// run before it; and the world as it ended.
    fn recorded_with(
        mut app: App,
        frames: &[Duration],
        mut between: impl FnMut(&mut Stack, &mut Context, u64),
    ) -> (String, u64, World) {
        let path = std::env::temp_dir().join(format!(
            "gg-engine-test-{}-{:?}.recording",
            std::process::id(),
            std::thread::current().id()
        ));
        let recorder = Recorder::create(&path, app.name).expect("created");
        app.stack.record(recorder);
        let mut cx = Context::new(None);
        app.stack.attach(&mut cx);
        let mut stepper = Stepper::default();
        for &dt in frames {
            between(&mut app.stack, &mut cx, stepper.ticks);
            stepper.step(dt, |tick| app.stack.tick(&mut cx, tick), || Duration::ZERO);
            app.stack.update(&mut cx, dt);
        }
        let ended = cx.world().clone();
        app.stack.detach(&mut cx);
        app.stack.recorded().expect("written");
        let text = std::fs::read_to_string(&path).expect("written");
        std::fs::remove_file(&path).expect("removed");
        (text, stepper.ticks, ended)
    }

    /// A replay of `text` by `app` to its end: the ticks it ran and its world's hash, or the ticks
    /// it ran before the error it stopped at.
    fn replay_all(app: App, text: &str) -> Result<(u64, WorldHash), (u64, gg_core::Error)> {
        let mut replay = Replay::new(app, text).map_err(|error| (0, error))?;
        let mut ticks = 0;
        loop {
            match replay.step() {
                Ok(Some(tick)) => {
                    assert_eq!(tick.number, ticks);
                    ticks += 1;
                }
                Ok(None) => return Ok((ticks, replay.world().hash())),
                Err(error) => return Err((ticks, error)),
            }
        }
    }

    /// `text` with its line `at`, from 0, in place of `line`.
    fn changed(text: &str, at: usize, line: &str) -> String {
        let mut lines: Vec<&str> = text.lines().collect();
        lines[at] = line;
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    /// A run recorded as it goes at a frame rate of its own, of a layer whose commands its updates
    /// queue, under one without, replays without a frame to the hashes it recorded, and to the
    /// world it ended with; a copy of it with a command changed, or a hash, goes another way at
    /// that tick; and a copy cut off partway through a line replays its whole lines.
    #[test]
    fn a_recorded_run_replays_to_its_hashes() {
        let app = two_layers;
        let ms = Duration::from_millis;
        // Some frames run no tick, and some several.
        let frames = [ms(1), ms(10), ms(40), Tick::DT, ms(2), ms(48), ms(130)].repeat(6);
        let (text, ticks, ended) = recorded(app(), &frames);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[3], "layers Queuer Turner", "{text}");
        assert_eq!(lines.last(), Some(&"end"), "{text}");
        let commands = lines.iter().filter(|line| line.starts_with("> 0 ")).count();
        assert_eq!(
            commands,
            1 + 42 / 3 - 1,
            "the last update's waits for a tick: {text}"
        );

        let replayed = |text: &str| replay_all(app(), text);
        assert_eq!(replayed(&text).ok(), Some((ticks, ended)));
        assert_eq!(Replay::rewrite(app(), &text).expect("rewritten"), text);

        let tick_of = |line: &str| line.split_once(' ')?.0.parse::<u64>().ok();
        let changed = |at: usize, line: &str| changed(&text, at, line);
        // The last command's tick, which its next hash is of.
        let command = lines.iter().rposition(|line| line.starts_with("> 0 "));
        let command = command.expect("a command");
        let hash = lines[command..]
            .iter()
            .position(|line| tick_of(line).is_some());
        let hash = command + hash.expect("its tick's hash");
        let tick = tick_of(lines[hash]).expect("a tick");
        let (ran, error) = replayed(&changed(command, "> 0 Struct([(\"0\", F32(9.0))])"))
            .expect_err("another way");
        assert_eq!((ran, error.code()), (tick, "replay.diverged"), "{error}");
        let layout = lines[hash].rsplit(' ').next().expect("a layout");
        let rehashed = format!("{tick} 0000000000000000 {layout}");
        let (ran, error) = replayed(&changed(hash, &rehashed)).expect_err("another way");
        assert_eq!((ran, error.code()), (tick, "replay.diverged"), "{error}");
        let says = format!("the world's content goes another way after tick {tick}");
        assert!(error.to_string().contains(&says), "{error}");
        let rewritten = Replay::rewrite(app(), &changed(hash, &rehashed)).expect("rewritten");
        assert_eq!(rewritten, text);
        // Cut off in its hash, the tick runs its commands, unchecked.
        let whole: String = lines[..hash]
            .iter()
            .map(|line| format!("{line}\n"))
            .collect();
        let cut = whole + &lines[hash][..10];
        let (ticks, _) = replayed(&cut).unwrap_or_else(|(_, error)| panic!("{error}"));
        assert_eq!(ticks, tick + 1);
    }

    /// A layer's actions, fed key events between frames of a rate of their own, change at the
    /// first tick after the events come, as the commands its event queues: an axis is held at every
    /// tick until its release, from either key, a tap shorter than a frame applies once, and a
    /// repeat, a release without its press and a key not bound queue none; and the run replays to
    /// its hashes.
    #[test]
    fn actions_change_at_the_first_tick_after_their_events() {
        let ms = Duration::from_millis;
        let key = |key, pressed| Event::Key {
            key,
            pressed,
            repeat: false,
        };
        let repeat = Event::Key {
            key: Key::ArrowRight,
            pressed: true,
            repeat: true,
        };
        // After how many ticks each event comes, and whether the layer queues a command for it:
        // not for Space's release, which it doesn't apply.
        let script = [
            (4, key(Key::ArrowRight, true), true),
            (6, repeat, false),
            (12, key(Key::ArrowRight, false), true),
            (20, key(Key::Space, true), true),
            (20, key(Key::Space, false), false),
            (24, key(Key::Space, false), false),
            (30, key(Key::ArrowLeft, true), true),
            (34, key(Key::KeyW, true), false),
            (40, Event::Focused(false), true),
        ];
        let app = |applied| App::new("Piloted").layer(Pilot::new(applied));
        for frame in [ms(1), ms(10), ms(40), ms(130)] {
            let count = (Tick::DT * 50).as_nanos().div_ceil(frame.as_nanos());
            let frames = vec![frame; count as usize];
            let applied = Rc::new(RefCell::new(Vec::new()));
            let (mut next, mut arrived) = (0, Vec::new());
            let between = |stack: &mut Stack, cx: &mut Context, ran| {
                while let Some(&(after, event, changes)) = script.get(next)
                    && after <= ran
                {
                    let _ = stack.event(cx, &event);
                    if changes {
                        arrived.push(ran);
                    }
                    next += 1;
                }
            };
            let (text, ticks, world) = recorded_with(app(applied.clone()), &frames, between);
            assert_eq!(next, script.len(), "each event came, at {frame:?}");
            assert_eq!(*applied.borrow(), arrived, "at {frame:?}");
            let [pushed, let_go, _, left, unfocused] = arrived[..] else {
                panic!("{arrived:?} at {frame:?}");
            };
            let piloted = world.query::<&Piloted>().next().copied();
            let piloted = piloted.map(|p| (p.push, u64::from(p.pushed), p.taps));
            let pushed = let_go - pushed + unfocused - left;
            assert_eq!(piloted, Some((0.0, pushed, 1)), "at {frame:?}");
            let replayed = replay_all(app(Rc::default()), &text);
            assert_eq!(replayed.ok(), Some((ticks, world.hash())), "at {frame:?}");
        }
    }

    /// A recording of an app without layers, as the player is, replays too.
    #[test]
    fn a_recording_of_no_layers_replays() {
        let (text, ticks, ended) = recorded(App::new("Empty"), &[Tick::DT; 3]);
        assert_eq!(text.lines().nth(3), Some("layers "));
        let mut replay = Replay::new(App::new("Empty"), &text).unwrap_or_else(|e| panic!("{e}"));
        let mut ran = 0;
        while replay
            .step()
            .unwrap_or_else(|error| panic!("{error}"))
            .is_some()
        {
            ran += 1;
        }
        assert_eq!((ran, replay.world().hash()), (ticks, ended));
    }

    /// Each line a recording can get wrong is refused with its code, and, but for what's wrong with
    /// the recording as a whole, its line; and a value's column is the line's.
    #[test]
    fn a_recording_is_refused_where_it_goes_wrong() {
        let (text, ..) = recorded(two_layers(), &[Tick::DT; 4]);
        let lines: Vec<&str> = text.lines().collect();
        let command = lines.iter().position(|line| line.starts_with("> 0 "));
        let command = command.expect("the command its attach queued");
        let second = lines.iter().position(|line| line.starts_with("1 "));
        let second = second.expect("tick 1's hash");
        let hash = lines[second].split_once(' ').expect("a hash").1;
        let bad_value = "> 0 Struct([(\"0\", F32(1.0)]";
        let after_end = format!("{text}0 {hash}\n");
        let cases = [
            (changed(&text, 0, "gg-recording 2"), "replay.format", None),
            (changed(&text, 0, "(entities: {})"), "replay.format", None),
            (
                lines[..3].iter().map(|line| format!("{line}\n")).collect(),
                "replay.format",
                None,
            ),
            (changed(&text, 1, "app Other"), "replay.format", None),
            (changed(&text, 3, "layers Queuer"), "replay.format", None),
            (changed(&text, 4, "attach"), "replay.syntax", Some(5)),
            (changed(&text, 4, "seed 12"), "replay.syntax", Some(5)),
            (changed(&text, 5, "attach 00 00"), "replay.syntax", Some(6)),
            (
                changed(&text, second, &format!("2 {hash}")),
                "replay.syntax",
                Some(second + 1),
            ),
            (
                changed(&text, second, "1 tick"),
                "replay.syntax",
                Some(second + 1),
            ),
            (after_end.clone(), "replay.syntax", Some(lines.len() + 1)),
            (
                changed(&text, command, bad_value),
                "replay.syntax",
                Some(command + 1),
            ),
            (
                changed(&text, command, "> 2 Opaque"),
                "replay.bad_command",
                Some(command + 1),
            ),
            (
                changed(&text, command, "> 1 Opaque"),
                "replay.bad_command",
                Some(command + 1),
            ),
            (
                changed(&text, command, "> 0 UInt(1)"),
                "replay.bad_command",
                Some(command + 1),
            ),
        ];
        for (text, code, line) in cases {
            let replayed = Replay::new(two_layers(), &text).and_then(|mut replay| {
                while replay.step()?.is_some() {}
                Ok(())
            });
            let error = replayed.expect_err(&text);
            let message = error.to_string();
            assert_eq!(error.code(), code, "{message}\n{text}");
            if let Some(line) = line {
                assert!(message.starts_with(&format!("line {line}: ")), "{message}");
            }
        }
        // The value's column, from the start of its line, after `> 0 `.
        let alone = recording::text::Text::parse(&bad_value[4..], 1).expect_err("refused");
        let alone = alone.to_string();
        let column = |message: &str| {
            let column = message.split("column ").nth(1).expect("a column");
            column
                .split(':')
                .next()
                .expect("a number")
                .parse::<usize>()
                .expect("a number")
        };
        let text = changed(&text, command, bad_value);
        let error = Replay::new(two_layers(), &text).err().expect("refused");
        assert_eq!(column(&error.to_string()), column(&alone) + 4, "{error}");
    }

    /// A layer's command that its replay would refuse, as one holding something opaque, which no
    /// value makes, stops the recording before it, which fails, naming the layer, the command and
    /// where in it what's opaque is; what was written before it replays.
    #[test]
    fn a_command_that_wont_replay_stops_the_recording() {
        let placed = || {
            App::new("Placed")
                .layer(Turner(Rc::default()))
                .layer(Placer::default())
        };
        let mut app = placed();
        let path = std::env::temp_dir().join(format!(
            "gg-engine-test-{}-{:?}-placed.recording",
            std::process::id(),
            std::thread::current().id()
        ));
        app.stack
            .record(Recorder::create(&path, app.name).expect("created"));
        let mut cx = Context::new(None);
        app.stack.attach(&mut cx);
        app.stack.tick(&mut cx, Tick { number: 0 });
        app.stack.detach(&mut cx);
        let error = app.stack.recorded().expect_err("stopped");
        let text = std::fs::read_to_string(&path).expect("written");
        std::fs::remove_file(&path).expect("removed");
        assert_eq!(error.code(), "record.bad_command", "{error}");
        let message = error.to_string();
        for named in [
            "layer 1, Placer,",
            "(\"texture\", Opaque)",
            "at `0.texture`: it's opaque",
        ] {
            assert!(message.contains(named), "{named}: {message}");
        }
        // Its header and its attach's hash alone: neither the command, nor the tick's hash after
        // it, nor an end.
        let last = text.lines().last().unwrap_or_default();
        assert!(last.starts_with("attach "), "{text}");
        let replayed = replay_all(placed(), &text).map(|(ticks, _)| ticks);
        assert_eq!(replayed.map_err(|(_, error)| error.code()), Ok(0), "{text}");
    }

    /// `--replay` checks every hash to the end, and stops where `--dump` says, failing where a
    /// hash goes another way, or where no tick is the one it names.
    #[test]
    fn a_replay_runs_to_its_end_or_its_dump() {
        let (text, ..) = recorded(two_layers(), &[Tick::DT; 4]);
        let run = |text: &str, dump| {
            let mut replay = Replay::new(two_layers(), text).expect("attached");
            let ran = replay.run(dump).map_err(|error| error.code());
            (ran, replay.step().ok().flatten().map(|tick| tick.number))
        };
        assert_eq!(run(&text, None), (Ok(()), None));
        // Stopped after tick 1, it goes on from tick 2.
        assert_eq!(run(&text, Some(args::Dump::Tick(1))), (Ok(()), Some(2)));
        assert_eq!(run(&text, Some(args::Dump::Attach)), (Ok(()), Some(0)));
        assert_eq!(
            run(&text, Some(args::Dump::Tick(4))).0,
            Err("replay.no_tick")
        );
        let lines: Vec<&str> = text.lines().collect();
        let at = lines.iter().position(|line| line.starts_with("2 "));
        let at = at.expect("tick 2's hash");
        let layout = lines[at].rsplit(' ').next().expect("a layout");
        let wrong = changed(&text, at, &format!("2 0000000000000000 {layout}"));
        assert_eq!(run(&wrong, None).0, Err("replay.diverged"));
        assert_eq!(run(&wrong, Some(args::Dump::Tick(1))).0, Ok(()));
        assert_eq!(
            run(&wrong, Some(args::Dump::Tick(2))).0,
            Err("replay.diverged")
        );
    }

    #[test]
    fn a_frame_runs_at_most_8_ticks_keeping_the_fraction() {
        let mut stepper = Stepper::default();
        let fast = || Duration::ZERO;
        // 64.32 ticks' time: 8 run, the other 56 whole ticks are dropped, and 5 ms is left over.
        assert_eq!(stepper.step(Duration::from_millis(1005), |_| {}, fast), 8);
        assert_eq!(stepper.dropped, 56);
        // A whole tick, with the 5 ms.
        assert_eq!(stepper.step(Duration::from_micros(10_625), |_| {}, fast), 1);
        assert_eq!(stepper.step(Duration::ZERO, |_| {}, fast), 0);
        assert_eq!((stepper.ticks, stepper.dropped), (9, 56));
    }

    #[test]
    fn ticks_that_run_slow_stop_the_frame() {
        let mut stepper = Stepper::default();
        let mut numbers = Vec::new();
        let ran = Cell::new(0);
        for _ in 0..2 {
            ran.set(0);
            // Each tick takes two ticks' time.
            let took = || Tick::DT * 2 * ran.get();
            let owed = Tick::DT * MOST_TICKS as u32;
            let tick = |tick: Tick| {
                ran.set(ran.get() + 1);
                numbers.push(tick.number);
            };
            assert_eq!(stepper.step(owed, tick, took), 1);
        }
        assert_eq!(numbers, [0, 1]);
        assert_eq!(stepper.dropped, 2 * (MOST_TICKS - 1));
    }

    #[test]
    fn headless_frames_tick_once_before_the_layers_update() {
        let mut stepper = Stepper::lockstep();
        for dt in [Duration::ZERO, Duration::from_secs(1)] {
            assert_eq!(stepper.step(dt, |_| {}, || Duration::from_secs(1)), 1);
        }
        assert_eq!((stepper.ticks, stepper.dropped), (2, 0));

        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut stack = Stack::default();
        stack.push_layer(Order(calls.clone()));
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            ..Default::default()
        };
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        let mut clock = Clock::default();
        let mut stepper = Stepper::lockstep();
        let egui = egui_context();
        for _ in 0..2 {
            let (cx, clock, stepper) = (&mut cx, &mut clock, &mut stepper);
            let dt = clock.start_frame();
            let ui = Some((&egui, &mut dock, input()));
            let (mut output, _) = run_frame(&mut stack, cx, dt, stepper, ui).expect("a pass");
            // As the renderer would have.
            output.textures_delta.clear();
        }
        assert_eq!(*calls.borrow(), ["tick 0", "update", "tick 1", "update"]);
    }

    #[test]
    fn the_world_draws_over_the_layers_draws() {
        let mut stack = Stack::default();
        stack.push_layer(Drawer);
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            ..Default::default()
        };
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        let mut clock = Clock::default();
        let mut stepper = Stepper::default();
        let egui = egui_context();
        let (cx, dt, stepper) = (&mut cx, clock.start_frame(), &mut stepper);
        let ui = Some((&egui, &mut dock, input));
        let (mut output, _) = run_frame(&mut stack, cx, dt, stepper, ui).expect("a pass");
        // As the renderer would have.
        output.textures_delta.clear();
        // The layer's sprite, of no entity, then the world's, with its entity's ID.
        let world = cx.world();
        let red_one = world.query::<(Entity, &Sprite)>().next();
        let (red_one, _) = red_one.expect("the sprite the layer spawned");
        let id = world.id(red_one).expect("alive").get();
        let mut expected = Draws::default();
        expected.push_sprite(Mat4::IDENTITY, Sprite::default());
        let red_one = (Transform::IDENTITY.matrix(), red(), id);
        expected.extend_sprites(iter::once(red_one));
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        let mut frame = render_thread::Extracted::default();
        cx.extract(&mut frame, viewport);
        assert_eq!(frame.draws, expected);
        // The context's buffer and the one the frame's draws come back in both start empty.
        for _ in 0..2 {
            cx.extract(&mut frame, viewport);
            assert!(
                frame.draws.is_empty(),
                "the context keeps none of a frame's draws for the next"
            );
        }
    }

    /// Spawns a camera as it attaches, and sets a viewpoint of its own as it updates while `own`
    /// says to.
    struct Overlooker {
        own: Rc<Cell<bool>>,
    }

    /// What [`Overlooker`] sets.
    const OVERLOOKING: Viewpoint = Viewpoint {
        position: Vec3::new(0.0, 0.0, 5.0),
        rotation: Quat::IDENTITY,
        projection: Projection::Orthographic {
            height: 3.0,
            near: -1.0,
            far: 1.0,
        },
    };

    /// What [`Overlooker`]'s camera sees from.
    const ITS_CAMERA: Viewpoint = Viewpoint {
        position: Vec3::new(1.0, 2.0, 0.0),
        rotation: Quat::IDENTITY,
        projection: Projection::Orthographic {
            height: 6.0,
            near: -1.0,
            far: 1.0,
        },
    };

    impl Layer for Overlooker {
        fn attach(&mut self, cx: &mut Context) {
            let at = Transform {
                translation: ITS_CAMERA.position,
                ..Transform::IDENTITY
            };
            let camera = Camera {
                projection: ITS_CAMERA.projection,
                primary: true,
            };
            cx.world_mut().spawn((at, camera));
        }

        fn update(&mut self, cx: &mut Context, _: Duration) {
            if self.own.get() {
                cx.set_viewpoint(OVERLOOKING);
            }
        }
    }

    /// The view sees from the default without a camera; through the world's camera, as it draws
    /// the world, in frames no layer sets a viewpoint; and from a layer's viewpoint in the frame
    /// it sets it, that frame alone.
    #[test]
    fn the_view_sees_from_a_layers_viewpoint_for_its_frame_and_else_through_the_worlds_camera() {
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        let mut frame = render_thread::Extracted::default();
        let own = Rc::new(Cell::new(false));
        let mut stack = Stack::default();
        stack.push_layer(Overlooker { own: own.clone() });
        let mut cx = Context::new(None);
        cx.extract(&mut frame, viewport);
        assert_eq!(frame.viewpoint, Viewpoint::default());
        stack.attach(&mut cx);
        let mut stepper = Stepper::lockstep();
        for (set, seen) in [
            (false, ITS_CAMERA),
            (true, OVERLOOKING),
            (false, ITS_CAMERA),
        ] {
            own.set(set);
            let dt = Duration::ZERO;
            assert!(run_frame(&mut stack, &mut cx, dt, &mut stepper, None).is_none());
            cx.extract(&mut frame, viewport);
            assert_eq!(frame.viewpoint, seen, "a viewpoint set: {set}");
        }
    }

    /// Frames between ticks draw the world on its way between them, from where the latest tick
    /// found it to where it left it, as far as they're past the tick: frames a quarter of a tick
    /// apart, and two and a half, which run two ticks or three, each copying the world as it
    /// starts. A frame before the first tick draws the world as it attached; an entity reset at a
    /// tick, and one spawned at one, draw where the tick left them.
    #[test]
    fn frames_draw_between_the_ticks_around_them() {
        let mut stack = Stack::default();
        stack.push_layer(Slider::default());
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let (mut stepper, egui) = (Stepper::default(), egui_context());
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        let quarters = iter::repeat_n(Tick::DT / 4, 16);
        let dts = quarters.chain(iter::repeat_n(Tick::DT * 5 / 2, 3));
        // How far each frame is past its last tick: a quarter more each frame, from the fourth,
        // whose tick is the first; then half a tick, none and half.
        let alphas = [0.0; 3]
            .into_iter()
            .chain((0..13).map(|frame| (frame % 4) as f32 / 4.0));
        let alphas = alphas.chain([0.5, 0.0, 0.5]);
        for (frame, (dt, alpha)) in (1..).zip(dts.zip(alphas)) {
            let ui = (&egui, &mut dock);
            let draws = drawn(&mut stack, &mut cx, dt, &mut stepper, ui);
            // Each tick moves the slider 1. The frames two and a half ticks apart run two ticks or
            // three, or fewer on a machine that stalls them past their time, as the stepper counts.
            let slider = match stepper.ticks {
                0 => 0.0,
                ticks => (ticks - 1) as f32 + alpha,
            };
            // Tick 2 runs in frame 12, and tick 3 in frame 16.
            let jumper = if frame < 16 { 10.0 } else { 100.0 };
            let want = slider_draws(cx.world(), slider, jumper, frame >= 12);
            assert_eq!(draws, want, "frame {frame}, after {} ticks", stepper.ticks);
        }
        stack.detach(&mut cx);
    }

    /// A headless frame, in lockstep, draws the world as its tick left it, bit for bit, as though
    /// nothing drew between ticks.
    #[test]
    fn a_headless_frame_draws_the_tick_it_ran() {
        let mut stack = Stack::default();
        stack.push_layer(Slider::default());
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let (mut stepper, egui) = (Stepper::lockstep(), egui_context());
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        for frame in 1..=5 {
            let ui = (&egui, &mut dock);
            let draws = drawn(&mut stack, &mut cx, Duration::ZERO, &mut stepper, ui);
            let jumper = if frame < 4 { 10.0 } else { 100.0 };
            let want = slider_draws(cx.world(), frame as f32, jumper, frame >= 3);
            assert_eq!(draws, want, "frame {frame}");
        }
        stack.detach(&mut cx);
    }

    #[test]
    fn the_inspector_lists_the_world_in_the_dock() {
        let mut stack = Stack::default();
        stack.push_layer(Drawer);
        stack.push_overlay(Inspector::default());
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1200.0),
            )),
            ..Default::default()
        };
        let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
        let mut clock = Clock::default();
        let mut stepper = Stepper::default();
        let egui = egui_context();
        let mut texts = Vec::new();
        // The dock places a panel in its first frame, and draws it from the next.
        for _ in 0..3 {
            let (cx, dt, stepper) = (&mut cx, clock.start_frame(), &mut stepper);
            let ui = Some((&egui, &mut dock, input()));
            let (mut output, _) = run_frame(&mut stack, cx, dt, stepper, ui).expect("a pass");
            output.textures_delta.clear();
            texts.clear();
            for clipped in output.shapes {
                find_texts(clipped.shape, &mut texts);
            }
        }
        for text in [
            "1 entity",
            "Nothing picked: pick an entity in the Entities panel",
        ] {
            assert!(texts.iter().any(|shown| shown == text), "{text}: {texts:?}");
        }
    }

    fn find_texts(shape: egui::Shape, texts: &mut Vec<String>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    find_texts(shape, texts);
                }
            }
            egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
            _ => {}
        }
    }

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

    #[test]
    fn the_games_view_is_the_central_area_in_pixels() {
        let central = egui::Rect::from_min_max(egui::pos2(10.2, 31.0), egui::pos2(400.6, 300.0));
        assert_eq!(
            viewport(central, 1.25),
            Viewport {
                origin: [13, 39],
                size: [488, 336],
            }
        );
    }

    #[test]
    fn layers_know_where_the_view_was_handed_off_while_it_had_an_area() {
        let mut cx = Context::new(None);
        assert_eq!(cx.view(), None);
        let mut frame = render_thread::Extracted::default();
        let shown = egui::Rect::from_min_max(egui::pos2(10.0, 40.0), egui::pos2(410.0, 340.0));
        cx.extract(&mut frame, viewport(shown, 1.5));
        let view = Viewport {
            origin: [15, 60],
            size: [600, 450],
        };
        assert_eq!(cx.view(), Some(view));
        // As while a tab shows over a dockspace's view, whose dock then has it nowhere.
        cx.extract(&mut frame, viewport(egui::Rect::NOTHING, 1.5));
        assert_eq!(cx.view(), None);
    }

    #[test]
    fn the_frame_after_a_pause_counts_from_its_own_start() {
        let mut clock = Clock::default();
        assert_eq!(clock.start_frame(), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));
        assert!(clock.start_frame() > Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));
        clock.resume();
        assert_eq!(clock.start_frame(), Duration::ZERO);
        // The run's time still counts the pause.
        assert!(clock.elapsed() >= Duration::from_millis(2));
    }
}
