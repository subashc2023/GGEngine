//! What apps build on: the main loop, window and input, scene, assets, and handing each
//! frame's render data to `gg-renderer`.
//!
//! An app's `main` starts the engine; there is no separate entry-point crate.

mod args;
mod chrome;
mod dock;
mod event;
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
use gg_renderer::{Renderer, Settings, Validation};
use tracing::{debug, error, info};

use args::Args;
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
/// `gg_engine::run(gg_engine::App::new("Sandbox").layer(Sandbox::default()))`.
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
            render_frame(&egui, &cx, output, &mut [], &mut renderer)?;
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
/// egui allocates as it lays out and tessellates, so frames that draw UI allocate; the UI is a
/// development overlay.
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

/// The second half: renders the frame with the UI in `output` over it, and into the windows of
/// their own what their passes drew, in `windows`. Returns whether the frame was presented, and
/// egui's requests of the main window, such as the cursor to show.
fn render_frame(
    egui: &egui::Context,
    cx: &Context,
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
    let presented = renderer.render(cx.clear_color(), ui, &mut windows)?;
    Ok((presented, output.platform_output))
}

/// Measures frames: the time between them, and since the first.
#[derive(Default)]
struct Clock {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Clock {
    /// Starts a frame, and returns the time since the last one started; zero for the first.
    fn tick(&mut self) -> Duration {
        let now = Instant::now();
        let dt = self.last.map_or(Duration::ZERO, |last| now - last);
        self.first.get_or_insert(now);
        self.last = Some(now);
        dt
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
