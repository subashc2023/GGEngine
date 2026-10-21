//! Logging through `tracing`: colored lines on the console for people, and every event as a JSON
//! line in a file for agents and tools.

mod console;

use std::backtrace::{Backtrace, BacktraceStatus};
use std::fs::File;
use std::io::{self, IsTerminal};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use tracing::level_filters::LevelFilter;
use tracing::{error, warn};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The engine's crates, which log debug events in debug builds unless `RUST_LOG` says otherwise.
const ENGINE_CRATES: [&str; 4] = ["gg_core", "gg_gpu", "gg_renderer", "gg_engine"];

/// Starts logging for this process: colored lines on stderr, and a JSON-lines file in `dir` named
/// after `name`, the time and the process ID. Returns the file's path, or `None` if the file
/// couldn't be created; the console still logs then, starting with a warning that says why.
///
/// `RUST_LOG` sets the filter, such as `debug` or `info,gg_gpu=trace`. Without it, events at info
/// and above are logged, and the engine's debug events too in debug builds. `NO_COLOR` turns the
/// console's colors off, and `CLICOLOR_FORCE` turns them on when stderr isn't a terminal.
///
/// Panics are logged as errors, with where they happened, before the process ends.
///
/// # Panics
///
/// If logging has already started: a process has one log.
pub fn init(name: &str, dir: &Path) -> Option<PathBuf> {
    let start = Instant::now();
    let (filter, bad_filter) = filter();
    let console = tracing_subscriber::fmt::layer()
        .event_format(console::Console::new(start, colors()))
        // Span fields are stored plain; the console format adds its own colors.
        .with_ansi(false)
        .with_writer(io::stderr);
    let (file, json) = match create(name, dir) {
        Ok((path, file)) => {
            let json = tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(true)
                .with_file(true)
                .with_line_number(true)
                .with_thread_names(true)
                .with_writer(Mutex::new(file));
            (Ok(path), Some(json))
        }
        Err(failure) => (Err(failure), None),
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(json)
        .init();
    std::panic::set_hook(Box::new(log_panic));

    if let Some(value) = bad_filter {
        warn!(
            code = "log.filter",
            value,
            "RUST_LOG isn't a filter, so the default applies; it takes the form `debug` or `info,gg_gpu=trace`"
        );
    }
    match file {
        Ok(path) => Some(path),
        Err((path, error)) => {
            warn!(
                code = "log.file",
                path = %path.display(),
                %error,
                "Can't create the JSON log, so only the console logs"
            );
            None
        }
    }
}

/// The filter `RUST_LOG` sets, or the default plus `RUST_LOG`'s value if it doesn't parse.
fn filter() -> (Targets, Option<String>) {
    let engine = if cfg!(debug_assertions) {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let default = Targets::new()
        .with_default(LevelFilter::INFO)
        .with_targets(ENGINE_CRATES.map(|target| (target, engine)));
    match std::env::var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => match value.parse() {
            Ok(filter) => (filter, None),
            Err(_) => (default, Some(value)),
        },
        _ => (default, None),
    }
}

/// Whether the console gets colors: stderr is a terminal that takes ANSI escape codes and
/// `NO_COLOR` isn't set, or `CLICOLOR_FORCE` asks for them.
fn colors() -> bool {
    let set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if set("NO_COLOR") {
        return false;
    }
    let forced =
        std::env::var_os("CLICOLOR_FORCE").is_some_and(|value| !value.is_empty() && value != "0");
    forced || (io::stderr().is_terminal() && ansi_codes_work())
}

#[cfg(windows)]
fn ansi_codes_work() -> bool {
    // Older Windows consoles show escape codes as text until asked to interpret them.
    nu_ansi_term::enable_ansi_support().is_ok()
}

#[cfg(not(windows))]
fn ansi_codes_work() -> bool {
    true
}

/// Creates `<dir>/<name>-<UTC time>-<process ID>.jsonl`, or returns the path it tried with the
/// reason it failed.
fn create(name: &str, dir: &Path) -> Result<(PathBuf, File), (PathBuf, io::Error)> {
    let mut time = String::new();
    // `2026-10-21T00:16:49.123456Z` becomes `20261021T001649`.
    let _ = SystemTime.format_time(&mut Writer::new(&mut time));
    let time: String = time
        .chars()
        .take(19)
        .filter(|&c| c.is_ascii_digit() || c == 'T')
        .collect();
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let path = dir.join(format!("{name}-{time}Z-{}.jsonl", std::process::id()));
    match std::fs::create_dir_all(dir).and_then(|()| File::create_new(&path)) {
        Ok(file) => Ok((path, file)),
        Err(error) => Err((path, error)),
    }
}

/// Logs a panic as an error, with where it happened, so it reaches the JSON log too.
fn log_panic(info: &PanicHookInfo<'_>) {
    let message = info
        .payload_as_str()
        .unwrap_or("(a panic whose payload isn't a string)");
    let location = info.location().map(tracing::field::display);
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("unnamed");
    let backtrace = Backtrace::capture();
    if matches!(backtrace.status(), BacktraceStatus::Captured) {
        error!(target: "panic", code = "panic", location, thread, %backtrace, "{message}");
    } else {
        error!(
            target: "panic",
            code = "panic",
            location,
            thread,
            backtrace = "off; set RUST_BACKTRACE=1 to capture one",
            "{message}"
        );
    }
}
