//! What apps build on: the main loop, window and input, layers and their UI, and handing each
//! frame's render data to `gg-renderer`; the scene and assets to come.
//!
//! An app's `main` starts the engine; there is no separate entry-point crate.

mod args;
mod camera;
mod chrome;
mod dock;
mod event;
mod image;
mod input;
mod layer;
mod viewports;
mod window;
// Windows' own frame around a window whose title bar the UI draws needs a window procedure in
// front of winit's, which only unsafe code installs: this module alone may use it.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows_frame;

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
/// The UI library layers draw with, in [`Layer::ui`]; apps use it from here, so that they
/// share the engine's version.
pub use egui;
pub use event::{Event, Key, MouseButton, Scroll};
/// The engine's vectors, matrices and quaternions; apps use them from here, so that they share
/// the engine's version.
pub use gg_core::math;
/// What layers draw, which they add and draw through their [`Context`]: sprites; meshes; shaders
/// for their vertex types, each added with how it blends; the textures those shaders and sprites
/// sample, and the samplers they sample them with; and the camera the game's view sees them from.
pub use gg_renderer::{Blend, Camera, Mesh, Projection, Sampler, Shader, Sprite, Texture};
pub use input::Input;
use layer::Stack;
pub use layer::{Context, Layer, Window};
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
        Ok(ran) => ran.report(),
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
    app.stack.detach(&mut cx);
    rendered?;
    Ok(Ran::finish(renderer, frames, clock.elapsed()))
}

/// egui, without its shortcut that closes the window, Ctrl or Cmd+Q: a game may use those keys,
/// and apps quit from their own menus.
fn egui_context() -> egui::Context {
    let egui = egui::Context::default();
    egui.options_mut(|options| options.quit_shortcuts.clear());
    egui
}

/// The first half of a turn of the main loop: updates the layers and has them draw their UI, in
/// the dock and with egui. [`render_frame`] renders what egui made of it.
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
    stack.update(cx, clock.tick());
    egui.run_ui(input, |root| {
        dock.begin(root);
        stack.ui(cx, &mut Ui::new(root, dock));
        dock.end(root);
    })
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
    let primitives = egui.tessellate(output.shapes, output.pixels_per_point);
    let ui = gg_renderer::Ui {
        primitives: &primitives,
        textures: &mut output.textures_delta,
        pixels_per_point: output.pixels_per_point,
    };
    let mut windows: Vec<_> = windows.iter_mut().map(Drawn::ui).collect();
    let data = cx.render_data(viewport(view, output.pixels_per_point));
    let presented = renderer.render(data, ui, &mut windows);
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

    /// Has the next frame count from its own start, as the first does, after a pause in which no
    /// frames ran, such as while the window was minimized: otherwise the layers would take the
    /// pause for one long frame.
    fn resume(&mut self) {
        self.last = None;
    }

    fn elapsed(&self) -> Duration {
        self.first.map_or(Duration::ZERO, |first| first.elapsed())
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
    time: Duration,
    validation: Validation,
}

impl Ran {
    /// Shuts the renderer down first, so that what validation reports on the way, such as
    /// objects left undestroyed, counts too.
    fn finish(renderer: Renderer, frames: u64, time: Duration) -> Self {
        let validation = renderer.validation();
        drop(renderer);
        Self {
            frames,
            time,
            validation,
        }
    }

    fn report(self) -> ExitCode {
        let messages = self.validation.messages();
        if messages > 0 {
            error!(
                code = "engine.validation",
                messages,
                "Vulkan validation reported {messages} problem(s); the warnings and errors above say what"
            );
            return ExitCode::FAILURE;
        }
        let seconds = self.time.as_secs_f64();
        let fps = if seconds > 0.0 {
            self.frames as f64 / seconds
        } else {
            0.0
        };
        info!(
            frames = self.frames,
            seconds = (seconds * 100.0).round() / 100.0,
            fps = (fps * 10.0).round() / 10.0,
            validated = self.validation.enabled(),
            "Exited cleanly"
        );
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
