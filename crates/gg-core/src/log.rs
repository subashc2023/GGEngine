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

/// The target panics are logged under, which every filter lets through but one that names its
/// fields, such as `panic[{code}]=off`, which outranks the engine's own directive for it.
const PANICS: &str = "panic";

/// Starts logging for this process: colored lines on stderr, and a JSON-lines file in `dir` named
/// after `name`, the time and the process ID. Returns the file's path, or `None` if the file
/// couldn't be created; the console still logs then, starting with a warning that says why.
///
/// `RUST_LOG` sets the filter, such as `debug` or `info,gg_gpu=trace`. Without it, events at info
/// and above are logged, and the engine's debug events too in debug builds. A filter that names
/// targets alone, such as `gg_gpu=trace`, sets their levels and keeps that default for the rest.
/// `NO_COLOR` turns the console's colors off, and `CLICOLOR_FORCE` turns them on when stderr isn't
/// a terminal.
///
/// Panics are logged as errors, `log.panic`, with where they happened, before the process ends,
/// even under `off`.
///
/// # Panics
///
/// If logging has already started: a process has one log.
pub fn init(name: &str, dir: &Path) -> Option<PathBuf> {
    let start = Instant::now();
    let rust_log = std::env::var("RUST_LOG").ok();
    let (filter, bad_filter) = filter(rust_log.as_deref());
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

/// The filter `RUST_LOG`'s value sets, or the default plus the value if it doesn't parse. A value
/// with a level for every target, such as `debug` or `info,gg_gpu=trace`, replaces the default;
/// one that names targets alone, such as `gg_gpu=trace`, sets their levels over the default's, so
/// that it adds detail and hides nothing. Panics pass it, as [`PANICS`] says.
fn filter(rust_log: Option<&str>) -> (Targets, Option<&str>) {
    let engine = if cfg!(debug_assertions) {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let default = Targets::new()
        .with_default(LevelFilter::INFO)
        .with_targets(ENGINE_CRATES.map(|target| (target, engine)));
    let (filter, bad) = match rust_log {
        Some(value) if !value.trim().is_empty() => match value.parse::<Targets>() {
            Ok(filter) if filter.default_level().is_some() => (filter, None),
            // Parsing adds directives in turn, each replacing any before it for the same target,
            // so the value's go after the default's, which `Targets` writes in the syntax it
            // parses.
            Ok(_) => {
                let merged = format!("{default},{value}").parse();
                let merged = merged.expect("the default and the value each parse, so both do");
                (merged, None)
            }
            Err(_) => (default, Some(value)),
        },
        _ => (default, None),
    };
    (filter.with_target(PANICS, LevelFilter::ERROR), bad)
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

/// Logs a panic as an error, with where it happened, so it reaches the JSON log too: under
/// [`PANICS`], which the filter lets through, `off` included.
fn log_panic(info: &PanicHookInfo<'_>) {
    let message = info
        .payload_as_str()
        .unwrap_or("(a panic whose payload isn't a string)");
    let location = info.location().map(tracing::field::display);
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("unnamed");
    let backtrace = Backtrace::capture();
    if matches!(backtrace.status(), BacktraceStatus::Captured) {
        error!(target: PANICS, code = "log.panic", location, thread, %backtrace, "{message}");
    } else {
        error!(
            target: PANICS,
            code = "log.panic",
            location,
            thread,
            backtrace = "off; set RUST_BACKTRACE=1 to capture one",
            "{message}"
        );
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use super::*;

    /// The level the default logs the engine's crates at.
    const ENGINE: Level = if cfg!(debug_assertions) {
        Level::DEBUG
    } else {
        Level::INFO
    };

    #[test]
    fn targets_alone_set_their_levels_and_keep_the_default_for_the_rest() {
        let (targets, bad) = filter(Some("gg_gpu=trace,gg_renderer=warn"));
        assert_eq!(bad, None);
        assert!(targets.would_enable("gg_gpu::device", &Level::TRACE));
        assert!(!targets.would_enable("gg_renderer", &Level::INFO));
        assert!(targets.would_enable("gg_engine", &ENGINE));
        assert!(targets.would_enable("sandbox", &Level::INFO));
        assert!(!targets.would_enable("sandbox", &Level::DEBUG));
    }

    #[test]
    fn a_level_for_every_target_replaces_the_default() {
        let (targets, _) = filter(Some("info,gg_gpu=debug"));
        assert!(targets.would_enable("gg_gpu", &Level::DEBUG));
        assert!(!targets.would_enable("gg_engine", &Level::DEBUG));
        assert!(targets.would_enable("sandbox", &Level::INFO));
        let (targets, _) = filter(Some("off"));
        assert!(!targets.would_enable("gg_engine", &Level::ERROR));
        assert!(!targets.would_enable("sandbox", &Level::ERROR));
    }

    #[test]
    fn no_filter_hides_panics() {
        for value in [
            "off",
            "panic=off",
            "gg_gpu=trace",
            "error,panic=off",
            "gg_gpu=loud",
        ] {
            let (targets, _) = filter(Some(value));
            assert!(targets.would_enable(PANICS, &Level::ERROR), "{value}");
        }
    }

    #[test]
    fn without_a_filter_the_default_applies() {
        for value in [None, Some(" "), Some("gg_gpu=loud")] {
            let (targets, bad) = filter(value);
            assert_eq!(bad, value.filter(|value| !value.trim().is_empty()));
            assert!(targets.would_enable("gg_gpu", &ENGINE), "{value:?}");
            assert!(!targets.would_enable("gg_gpu", &Level::TRACE), "{value:?}");
            assert!(targets.would_enable("sandbox", &Level::INFO), "{value:?}");
            assert!(!targets.would_enable("sandbox", &Level::DEBUG), "{value:?}");
        }
    }
}
