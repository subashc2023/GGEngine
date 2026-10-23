//! What apps build on: the main loop, window and input, layers and their UI, the world of entities
//! and their components, which it draws, handing each frame's render data to `gg-renderer`;
//! reflection, which describes the components' types so that tools reach them by name, and the
//! Inspector, which edits them; and measuring each frame, for the log and the Performance panel.
//! Scenes and assets to come.
//!
//! An app's `main` starts the engine; there is no separate entry-point crate.

// So that the derives' code, which names `::gg_engine`, works here too.
extern crate self as gg_engine;

mod args;
mod camera;
mod chrome;
mod dock;
mod draw;
mod ecs;
mod event;
mod image;
mod input;
mod inspector;
mod layer;
mod perf;
pub mod reflect;
mod transform;
mod viewports;
mod window;
// Windows' own frame around a window whose title bar the UI draws needs a window procedure in
// front of winit's, which only unsafe code installs: this module alone may use it.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows_frame;

use std::mem;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gg_core::Result;
use gg_renderer::{Renderer, Settings, Validation, Viewport};
use tracing::{debug, error, info};

use args::Args;
pub use camera::OrthographicCameraController;
use chrome::Chrome;
use dock::Dock;
pub use dock::Ui;
pub use ecs::{
    Bundle, Component, Entities, Entity, EntityId, Parent, Query, QueryIter, QueryIterMut,
    ReadOnlyQuery, World, Writable,
};
/// The UI library layers draw with, in [`Layer::ui`]; apps use it from here, so that they
/// share the engine's version.
pub use egui;
pub use event::{Event, Key, MouseButton, Scroll};
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
/// sheets whose cells they show; meshes; shaders
/// for their vertex types, each added with how it blends; the textures those shaders and sprites
/// sample, and the samplers they sample them with; and the camera the game's view sees them from.
pub use gg_renderer::{
    Blend, Camera, Mesh, Projection, Sampler, Shader, Sprite, SpriteSheet, Texture,
};
pub use input::Input;
pub use inspector::Inspector;
use layer::Stack;
pub use layer::{Context, Layer, Window};
pub use perf::PerfPanel;
use perf::{Perf, Summary};
pub use reflect::Reflect;
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
}

impl App {
    /// `name` titles the window and names the log file.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            stack: Stack::default(),
            custom_title_bar: false,
        }
    }

    /// Adds a layer above the layers added before it, and below every overlay.
    #[must_use]
    pub fn layer(mut self, layer: impl Layer + 'static) -> Self {
        self.stack.push_layer(Box::new(layer));
        self
    }

    /// Adds an overlay above everything added before it.
    #[must_use]
    pub fn overlay(mut self, overlay: impl Layer + 'static) -> Self {
        self.stack.push_overlay(Box::new(overlay));
        self
    }

    /// Has the UI draw the frames of the app's windows instead of the OS, with the menus layers
    /// add through [`Ui::menu`] in the main window's title bar, and the tab bars of torn-off
    /// panels as their windows' title bars: for tools, which want the room. On macOS the OS keeps
    /// its buttons and edges. Games keep the OS's frame.
    #[must_use]
    pub fn custom_title_bar(mut self) -> Self {
        self.custom_title_bar = true;
        self
    }
}

/// Runs `app` until its window closes or `--frames` runs out, and returns the exit status.
///
/// It parses the flags every app shares, which `--help` lists, and starts logging. The status is
/// 0 on success, 1 on an error or a Vulkan validation message, and 2 on bad arguments.
pub fn run(mut app: App) -> ExitCode {
    let started = Instant::now();
    let mut argv = std::env::args_os();
    let program = argv
        .next()
        .and_then(|path| Some(Path::new(&path).file_stem()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| app.name.to_lowercase());
    let args = match args::parse(argv) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{}", args::usage(&program));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!(
                "error[{}]: {error}\n\n{}",
                error.code(),
                args::usage(&program)
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
        pid = std::process::id(),
        "Starting GGEngine"
    );
    if let Some(path) = log {
        info!(path = %path.display(), "Writing the JSON log");
    }
    debug!(
        headless = args.headless,
        frames = args.frames,
        gpu = args.gpu.as_deref(),
        validation = args.validation,
        "Options"
    );

    let ran = if args.headless {
        run_headless(&mut app, &args)
    } else {
        window::run(&mut app, &args)
    };
    match ran {
        Ok(ran) => ran.report(started, &args),
        Err(error) => {
            error!(code = error.code(), "{error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Renders offscreen, without a window, as fast as the GPU allows.
fn run_headless(app: &mut App, args: &Args) -> Result<Ran> {
    let (width, height) = (1280, 720);
    let mut renderer = Renderer::headless(&settings(app, args), width, height)?;
    let mut cx = Context::new(None);
    let mut clock = Clock::default();
    let egui = egui_context();
    // Headless runs neither read nor save the layout, so that they're reproducible, and have no
    // window to frame.
    let mut dock = Dock::new(None, Chrome::Os);
    let mut frames = 0;
    app.stack.attach(&mut cx);
    let rendered = (|| -> Result<()> {
        while args.frames.is_none_or(|limit| frames < limit) {
            // One point is one pixel, as on a display without scaling.
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                time: Some(clock.elapsed().as_secs_f64()),
                ..Default::default()
            };
            let output = run_frame(&mut app.stack, &mut cx, &mut clock, &egui, &mut dock, input);
            render_frame(
                &egui,
                &mut cx,
                dock.central(),
                output,
                &mut [],
                &mut renderer,
            )?;
            frames += 1;
        }
        Ok(())
    })();
    let stopping = Instant::now();
    let _zone = gg_core::zone!("shutdown");
    app.stack.detach(&mut cx);
    rendered?;
    let perf = mem::take(&mut cx.perf);
    Ok(Ran::finish(renderer, frames, &clock, perf, stopping))
}

/// egui, without its shortcut that closes the window, Ctrl or Cmd+Q: a game may use those keys,
/// and apps quit from their own menus.
fn egui_context() -> egui::Context {
    let egui = egui::Context::default();
    egui.options_mut(|options| options.quit_shortcuts.clear());
    egui
}

/// The first half of a turn of the main loop: starts the frame, for Tracy and the frame's
/// measure, updates the layers, draws the world, over what they drew, and has the layers draw
/// their UI, in the dock and with egui.
/// [`render_frame`] renders what egui made of it.
///
/// egui allocates as it lays out here, and as [`render_frame`] tessellates, so frames that draw
/// UI allocate; the UI is a development overlay.
fn run_frame(
    stack: &mut Stack,
    cx: &mut Context,
    clock: &mut Clock,
    egui: &egui::Context,
    dock: &mut Dock,
    input: egui::RawInput,
) -> egui::FullOutput {
    let dt = clock.tick();
    gg_core::profile::frame_mark();
    cx.perf.tick(dt);
    let _zone = gg_core::zone!();
    let start = Instant::now();
    stack.update(cx, dt);
    cx.draw_world();
    let updated = Instant::now();
    let output = {
        let _zone = gg_core::zone!("egui");
        egui.run_ui(input, |root| {
            dock.begin(root);
            stack.ui(cx, &mut Ui::new(root, dock));
            dock.end(root);
        })
    };
    cx.perf.ran(updated - start, updated.elapsed());
    output
}

/// The second half: renders the frame, with what the layers drew in the game's view, the main
/// window's central area `view` in points, and the UI in `output` over it; and into the windows
/// of their own what their passes drew, in `windows`. Returns whether the frame was
/// presented, and egui's requests of the main window, such as the cursor to show.
fn render_frame(
    egui: &egui::Context,
    cx: &mut Context,
    view: egui::Rect,
    mut output: egui::FullOutput,
    windows: &mut [Drawn],
    renderer: &mut Renderer,
) -> Result<(bool, egui::PlatformOutput)> {
    let _zone = gg_core::zone!();
    let start = Instant::now();
    let primitives = {
        let _zone = gg_core::zone!("tessellate UI");
        egui.tessellate(output.shapes, output.pixels_per_point)
    };
    let tessellated = start.elapsed();
    let ui = gg_renderer::Ui {
        primitives: &primitives,
        textures: &mut output.textures_delta,
        pixels_per_point: output.pixels_per_point,
    };
    let mut windows: Vec<_> = windows.iter_mut().map(Drawn::ui).collect();
    let data = cx.render_data(viewport(view, output.pixels_per_point));
    let presented = renderer.render(data, ui, &mut windows);
    let render = start.elapsed() - tessellated;
    let rendered = matches!(presented, Ok(true));
    let drew = renderer.drawn();
    gg_core::plot!("draws", drew.draws as f64);
    gg_core::plot!("instances", drew.instances as f64);
    cx.perf
        .rendered(tessellated, render, renderer.waited(), drew, rendered);
    cx.clear_draws();
    Ok((presented?, output.platform_output))
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
    fn tick(&mut self) -> Duration {
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

fn settings<'a>(app: &'a App, args: &'a Args) -> Settings<'a> {
    Settings {
        app_name: app.name,
        validation: args.validation,
        gpu: args.gpu.as_deref(),
        vsync: args.vsync,
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
    validation: Validation,
    /// The GPU it rendered on.
    gpu: String,
    /// When its first frame started, if one did.
    first_tick: Option<Instant>,
    shutdown: Duration,
    perf: Summary,
}

impl Ran {
    /// Shuts the renderer down first, so that what validation reports on the way, such as
    /// objects left undestroyed, counts too. `stopping` is when the run began to end, after its
    /// last frame, and `perf` what it measured of its frames.
    fn finish(
        renderer: Renderer,
        frames: u64,
        clock: &Clock,
        mut perf: Perf,
        stopping: Instant,
    ) -> Self {
        perf.finish(clock.since_last(stopping));
        let validation = renderer.validation();
        let gpu = renderer.gpu_name().to_owned();
        drop(renderer);
        Self {
            frames,
            validation,
            gpu,
            first_tick: clock.first,
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
            measured = perf.measured,
            percentile_frames = perf.percentile_frames,
            startup_ms = self
                .first_tick
                .map(|first| perf::millis(first.saturating_duration_since(started))),
            first_frame_ms = perf.first_frame,
            shutdown_ms = perf::millis(self.shutdown),
            frame_ms_p50 = perf.frame.p50,
            frame_ms_p95 = perf.frame.p95,
            frame_ms_p99 = perf.frame.p99,
            frame_ms_mean = perf.frame.mean,
            frame_ms_max = perf.frame.max,
            update_ms_mean = perf.update.mean,
            update_ms_p95 = perf.update.p95,
            ui_ms_mean = perf.ui.mean,
            ui_ms_p95 = perf.ui.p95,
            render_ms_mean = perf.render.mean,
            render_ms_p95 = perf.render.p95,
            wait_ms_mean = perf.wait.mean,
            wait_ms_p95 = perf.wait.p95,
            draws_max = perf.draws_max,
            instances_max = perf.instances_max,
            gpu = self.gpu.as_str(),
            build = BUILD,
            headless = args.headless,
            vsync = args.vsync,
            validated = self.validation.enabled(),
            "Frame times"
        );
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
    use gg_core::math::{Mat4, Vec4};
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

    #[test]
    fn the_world_draws_over_the_layers_draws() {
        let mut stack = Stack::default();
        stack.push_layer(Box::new(Drawer));
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            ..Default::default()
        };
        let mut dock = Dock::new(None, Chrome::Os);
        let mut clock = Clock::default();
        let egui = egui_context();
        let mut output = run_frame(&mut stack, &mut cx, &mut clock, &egui, &mut dock, input);
        // As the renderer would have.
        output.textures_delta.clear();
        let mut expected = Draws::default();
        expected.push_sprite(Mat4::IDENTITY, Sprite::default());
        expected.push_sprite(Transform::IDENTITY.matrix(), red());
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        assert_eq!(*cx.render_data(viewport).draws, expected);
    }

    #[test]
    fn the_inspector_lists_the_world_in_the_dock() {
        let mut stack = Stack::default();
        stack.push_layer(Box::new(Drawer));
        stack.push_overlay(Box::new(Inspector::default()));
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1200.0),
            )),
            ..Default::default()
        };
        let mut dock = Dock::new(None, Chrome::Os);
        let mut clock = Clock::default();
        let egui = egui_context();
        let mut texts = Vec::new();
        // The dock places a panel in its first frame, and draws it from the next.
        for _ in 0..3 {
            let mut output = run_frame(&mut stack, &mut cx, &mut clock, &egui, &mut dock, input());
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
    fn the_frame_after_a_pause_counts_from_its_own_start() {
        let mut clock = Clock::default();
        assert_eq!(clock.tick(), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));
        assert!(clock.tick() > Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));
        clock.resume();
        assert_eq!(clock.tick(), Duration::ZERO);
        // The run's time still counts the pause.
        assert!(clock.elapsed() >= Duration::from_millis(2));
    }
}
