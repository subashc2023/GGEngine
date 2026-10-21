//! What apps build on: the main loop, window and input, scene, assets, and handing each
//! frame's render data to `gg-renderer`.
//!
//! An app's `main` starts the engine; there is no separate entry-point crate.

mod args;
mod window;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gg_core::Result;
use gg_renderer::{Renderer, Settings, Validation};
use tracing::{debug, error, info};

use args::Args;

const BUILD: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

/// Describes an app to the engine. An app's `main` is
/// `gg_engine::run(gg_engine::App::new("Sandbox"))`.
pub struct App {
    name: &'static str,
}

impl App {
    /// `name` titles the window and names the log file.
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

/// Runs `app` until its window closes or `--frames` runs out, and returns the exit status.
///
/// It parses the flags every app shares, which `--help` lists, and starts logging. The status is
/// 0 on success, 1 on an error or a Vulkan validation message, and 2 on bad arguments.
pub fn run(app: App) -> ExitCode {
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
        run_headless(&app, &args)
    } else {
        window::run(&app, &args)
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
fn run_headless(app: &App, args: &Args) -> Result<Ran> {
    let mut renderer = Renderer::headless(&settings(app, args), 1280, 720)?;
    let start = Instant::now();
    let mut frames = 0;
    while args.frames.is_none_or(|limit| frames < limit) {
        renderer.render(clear_color(start.elapsed()))?;
        frames += 1;
    }
    Ok(Ran::finish(renderer, frames, start.elapsed()))
}

fn settings<'a>(app: &'a App, args: &'a Args) -> Settings<'a> {
    Settings {
        app_name: app.name,
        validation: args.validation,
        gpu: args.gpu.as_deref(),
    }
}

/// Until the engine draws scenes, each frame clears to a color that drifts slowly, so that a
/// running app visibly animates.
fn clear_color(time: Duration) -> [f32; 4] {
    let t = time.as_secs_f32() * 0.4;
    let wave = |phase: f32| 0.02 + 0.06 * (0.5 + 0.5 * (t + phase).sin());
    [wave(0.0), wave(2.1), wave(4.2), 1.0]
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
