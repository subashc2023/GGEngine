//! Logging through `tracing`: colored console lines for people, and JSON lines for agents and
//! tools, in a file and in [`EVENTS`] for the tool API.

mod console;
// The OS's calls that walk the stack, behind a safe function.
#[allow(unsafe_code)]
mod frames;

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, IsTerminal};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tracing::level_filters::LevelFilter;
use tracing::{Level, Metadata, error, warn};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The engine's crates, logged at debug in debug builds unless `RUST_LOG` says otherwise.
const ENGINE_CRATES: [&str; 4] = ["gg_core", "gg_gpu", "gg_renderer", "gg_engine"];

/// Panics' target. Every filter passes it but one naming its fields, as `panic[{code}]=off` does,
/// outranking the engine's directive.
const PANICS: &str = "panic";

/// [`Events`]' limits: events, bytes of lines, and bytes of a line.
const MOST_EVENTS: usize = 10_000;
const MOST_BYTES: usize = 8 << 20;
const MOST_LINE: usize = 1 << 20;

/// The JSON log's last events.
pub static EVENTS: Events = Events::new();

/// Starts this process's logging: colored lines on stderr if `console`, and JSON lines in
/// [`EVENTS`] and a file in `dir`, named by `name`, the time and the process ID. Returns the file's
/// path, or `None`, warning why, if it can't be made.
///
/// Without `console`, nothing reaches stderr: an embedded game's tool reads its stderr and events,
/// and would show each event twice.
///
/// `RUST_LOG` sets the filter, such as `debug` or `info,gg_gpu=trace`. The default is info, and
/// debug for the engine's crates in debug builds. Targets alone, as `gg_gpu=trace`, keep it for the
/// rest. `NO_COLOR` turns colors off, and `CLICOLOR_FORCE` on off a terminal.
///
/// Panics log as errors (`log.panic`) before the process ends, even under `off`.
///
/// # Panics
///
/// - If logging has already started.
pub fn init(name: &str, dir: &Path, console: bool) -> Option<PathBuf> {
    let start = Instant::now();
    let rust_log = std::env::var("RUST_LOG").ok();
    let (filter, bad_filter) = filter(rust_log.as_deref());
    let console = console.then(|| {
        tracing_subscriber::fmt::layer()
            .event_format(console::Console::new(start, colors()))
            // Span fields are stored plain; the console format adds its own colors.
            .with_ansi(false)
            .with_writer(io::stderr)
    });
    let (file, created) = match create(name, dir) {
        Ok((path, file)) => (Some(Mutex::new(file)), Ok(path)),
        Err(failure) => (None, Err(failure)),
    };
    let json = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(false)
        .with_span_list(true)
        .with_file(true)
        .with_line_number(true)
        .with_thread_names(true)
        .with_writer(Tee {
            file,
            events: &EVENTS,
        });
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
    match created {
        Ok(path) => Some(path),
        Err((path, error)) => {
            warn!(
                code = "log.file",
                path = %path.display(),
                %error,
                "Can't create the JSON log, so only the console and the events kept in memory log"
            );
            None
        }
    }
}

/// `duration` in milliseconds, rounded to the microsecond, as events' `ms` fields give it.
pub fn millis(duration: Duration) -> f64 {
    (duration.as_nanos() as f64 / 1e3).round() / 1e3
}

/// The JSON log's last events, for any thread to [`read`](Self::read) as they come.
///
/// Events are numbered from 0. It keeps 10,000 events and 8 MiB of lines at most, the oldest going
/// first. A line over 1 MiB takes a number but isn't kept; one that isn't JSON, such as the layer's
/// "can't format" message, takes none. Keeping allocates each line, and nothing while nothing logs.
pub struct Events {
    ring: Mutex<Ring>,
    /// Wakes the waiting reads at each push.
    pushed: Condvar,
}

/// Oldest first; `end` is the next event's number.
struct Ring {
    kept: VecDeque<Kept>,
    bytes: usize,
    end: u64,
}

struct Kept {
    number: u64,
    level: Level,
    line: Arc<str>,
}

/// What [`Events::read`] read.
#[derive(Debug, PartialEq)]
pub struct Read {
    /// One past the last event looked at, of any level: where the next read goes on.
    pub next: u64,
    /// The next event's number as the read ended.
    pub end: u64,
    /// Events of any level from the start to `next` it couldn't read: dropped, or too long to keep.
    pub missed: u64,
    /// The lines read, in order, without line breaks.
    pub lines: Vec<Arc<str>>,
}

thread_local! {
    /// Whether this thread holds an [`Events`]' lock. What it logs meanwhile, as a panic there,
    /// isn't kept, rather than waiting forever for the lock.
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

/// Sets [`HOLDING`] until it drops, unwinding from a panic too.
struct Holding;

impl Holding {
    fn new() -> Self {
        HOLDING.set(true);
        Self
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        HOLDING.set(false);
    }
}

impl Events {
    pub const fn new() -> Self {
        Self {
            ring: Mutex::new(Ring {
                kept: VecDeque::new(),
                bytes: 0,
                end: 0,
            }),
            pushed: Condvar::new(),
        }
    }

    /// Keeps `line`, an event's JSON without its line break, at `level`, and wakes waiting reads.
    pub fn push(&self, level: Level, line: &str) {
        if !line.starts_with('{') || HOLDING.get() {
            return;
        }
        // Allocated before locking, so that no read waits for it.
        let line = (line.len() <= MOST_LINE).then(|| Arc::<str>::from(line));
        {
            let _holding = Holding::new();
            let mut ring = self.lock();
            let ring = &mut *ring;
            let number = ring.end;
            ring.end += 1;
            if let Some(line) = line {
                while ring.kept.len() >= MOST_EVENTS || ring.bytes + line.len() > MOST_BYTES {
                    let Some(oldest) = ring.kept.pop_front() else {
                        break;
                    };
                    ring.bytes -= oldest.line.len();
                }
                let bytes = line.len();
                ring.kept.push_back(Kept {
                    number,
                    level,
                    line,
                });
                ring.bytes += bytes;
            }
        }
        self.pushed.notify_all();
    }

    /// Reads kept events at `min` or more severe: from `from` (past the end, those to come), else
    /// the last `last`, else all, waiting up to `wait` for one. It reads at most `max_bytes` of
    /// lines but at least one, and the next read goes on from [`Read::next`].
    pub fn read(
        &self,
        from: Option<u64>,
        last: Option<usize>,
        min: Level,
        wait: Duration,
        max_bytes: usize,
    ) -> Read {
        // `tracing`'s levels order the more verbose after.
        let wanted = |kept: &Kept| kept.level <= min;
        let deadline = Instant::now().checked_add(wait);
        let _holding = Holding::new();
        let mut ring = self.lock();
        let oldest = ring.kept.front().map_or(ring.end, |kept| kept.number);
        let start = match (from, last) {
            (Some(from), _) => from.min(ring.end),
            (None, Some(0)) => ring.end,
            (None, Some(last)) => {
                let mut wanted_kept = ring.kept.iter().rev().filter(|kept| wanted(kept));
                wanted_kept.nth(last - 1).map_or(oldest, |kept| kept.number)
            }
            (None, None) => oldest,
        };
        let (mut next, mut looked, mut bytes) = (start, 0, 0);
        let mut lines = Vec::new();
        loop {
            let at = ring.kept.partition_point(|kept| kept.number < next);
            let mut full = false;
            for kept in ring.kept.range(at..) {
                if wanted(kept) {
                    if !lines.is_empty() && bytes + kept.line.len() > max_bytes {
                        full = true;
                        break;
                    }
                    bytes += kept.line.len();
                    lines.push(kept.line.clone());
                }
                looked += 1;
                next = kept.number + 1;
            }
            if full {
                break;
            }
            // Skips the events after the last kept, too long to keep.
            next = ring.end;
            let left = deadline.map_or(Duration::MAX, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if !lines.is_empty() || left.is_zero() {
                break;
            }
            // Pushes at other levels wake it too, as can nothing at all.
            ring = (self.pushed.wait_timeout(ring, left))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        Read {
            next,
            end: ring.end,
            missed: next - start - looked,
            lines,
        }
    }

    /// The ring, locked, even if poisoned: a [`push`](Self::push) cut short leaves it consistent,
    /// but for a number taken, which reads count as missed.
    fn lock(&self) -> MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

/// The JSON layer's writer, to the file, if made, and `events`.
struct Tee {
    file: Option<Mutex<File>>,
    events: &'static Events,
}

struct TeeWriter<'a> {
    file: Option<&'a Mutex<File>>,
    events: &'static Events,
    level: Level,
}

impl<'a> MakeWriter<'a> for Tee {
    type Writer = TeeWriter<'a>;

    /// Events come through [`make_writer_for`](Self::make_writer_for); what comes here, without an
    /// event's metadata, is kept at info.
    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter {
            file: self.file.as_ref(),
            events: self.events,
            level: Level::INFO,
        }
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Self::Writer {
        TeeWriter {
            level: *meta.level(),
            ..self.make_writer()
        }
    }
}

impl io::Write for TeeWriter<'_> {
    /// Takes all of `buf`, an event's whole line, so that each is kept whole. Fails only if the
    /// file does.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = match self.file {
            Some(file) => (file.lock())
                .unwrap_or_else(PoisonError::into_inner)
                .write_all(buf),
            None => Ok(()),
        };
        if let Ok(text) = std::str::from_utf8(buf) {
            self.events
                .push(self.level, text.strip_suffix('\n').unwrap_or(text));
        }
        written.map(|()| buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The filter `RUST_LOG`'s value sets, as [`init`] says, or the default and the bad value.
fn filter(rust_log: Option<&str>) -> (Targets, Option<&str>) {
    let engine = if cfg!(debug_assertions) {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let default = Targets::new()
        .with_default(LevelFilter::INFO)
        .with_targets(ENGINE_CRATES.map(|target| (target, engine)));
    // `Targets` doesn't trim: ` gg_gpu=trace` would name target ` gg_gpu`, and an empty directive,
    // from a stray comma, would set every target the value doesn't name to errors.
    let value = rust_log.map(|value| {
        let directives: Vec<&str> = (value.split(',').map(str::trim))
            .filter(|directive| !directive.is_empty())
            .collect();
        (value, directives.join(","))
    });
    let (filter, bad) = match value {
        Some((value, trimmed)) if !trimmed.is_empty() => match trimmed.parse::<Targets>() {
            Ok(filter) if filter.default_level().is_some() => (filter, None),
            // A directive replaces any before it for its target, so the value's go after the
            // default's, which `Targets` writes in the syntax it parses.
            Ok(_) => {
                let merged = format!("{default},{trimmed}").parse();
                let merged = merged.expect("the default and the value each parse, so both do");
                (merged, None)
            }
            Err(_) => (default, Some(value)),
        },
        _ => (default, None),
    };
    (filter.with_target(PANICS, LevelFilter::ERROR), bad)
}

/// Whether the console gets colors: never under `NO_COLOR`, else under `CLICOLOR_FORCE` or on a
/// terminal that takes ANSI escape codes.
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
    // Older Windows consoles print escape codes until told to interpret them.
    nu_ansi_term::enable_ansi_support().is_ok()
}

#[cfg(not(windows))]
fn ansi_codes_work() -> bool {
    true
}

/// Creates `<dir>/<name>-<UTC time>-<process ID>.jsonl`, or returns that path and the error.
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

/// Logs a panic as an error with its location, under [`PANICS`], so that even `off` passes it.
/// Its backtrace is captured whatever `RUST_BACKTRACE` says, as panics are rare and it explains
/// them. It resolves as far as the player's machine can (on Windows, without a PDB). Its
/// [`frames`] are for the build's kept symbols to resolve.
fn log_panic(info: &PanicHookInfo<'_>) {
    let message = info
        .payload_as_str()
        .unwrap_or("(a panic whose payload isn't a string)");
    let location = info.location().map(tracing::field::display);
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("unnamed");
    let frames = frames::capture();
    let backtrace = Backtrace::force_capture();
    error!(target: PANICS, code = "log.panic", location, thread, frames, %backtrace, "{message}");
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
        // A stray comma is no directive.
        for value in ["gg_gpu=trace,", ",gg_gpu=trace"] {
            let (targets, bad) = filter(Some(value));
            assert_eq!(bad, None, "{value}");
            assert!(targets.would_enable("gg_gpu", &Level::TRACE), "{value}");
            assert!(targets.would_enable("gg_engine", &ENGINE), "{value}");
            assert!(targets.would_enable("sandbox", &Level::INFO), "{value}");
        }
    }

    #[test]
    fn a_level_for_every_target_replaces_the_default() {
        // Spaces around a directive are no part of it.
        for value in ["info,gg_gpu=debug", "info, gg_gpu=trace"] {
            let (targets, bad) = filter(Some(value));
            assert_eq!(bad, None, "{value}");
            assert!(targets.would_enable("gg_gpu", &Level::DEBUG), "{value}");
            assert!(!targets.would_enable("gg_engine", &Level::DEBUG), "{value}");
            assert!(targets.would_enable("sandbox", &Level::INFO), "{value}");
        }
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

    fn line(n: usize) -> String {
        format!("{{\"n\":{n}}}")
    }

    fn lines(read: &Read) -> Vec<&str> {
        read.lines.iter().map(|line| &**line).collect()
    }

    /// Everything `events` keeps from `from`, without waiting.
    fn all_from(events: &Events, from: Option<u64>) -> Read {
        events.read(from, None, Level::TRACE, Duration::ZERO, usize::MAX)
    }

    #[test]
    fn the_oldest_events_go_past_the_most_kept_by_count_and_by_bytes() {
        let events = Events::new();
        for n in 0..MOST_EVENTS + 5 {
            events.push(Level::INFO, &line(n));
        }
        let read = all_from(&events, None);
        assert_eq!(read.lines.len(), MOST_EVENTS);
        assert_eq!(&*read.lines[0], line(5));
        let end = (MOST_EVENTS + 5) as u64;
        assert_eq!((read.next, read.end, read.missed), (end, end, 0));
        let read = all_from(&events, Some(2));
        assert_eq!((read.lines.len(), read.missed), (MOST_EVENTS, 3));

        // Eight of the longest lines kept fill the bytes kept, so a ninth pushes the first out.
        let events = Events::new();
        let long = |n: usize| format!("{{\"n\":{n},\"x\":\"{}\"}}", "x".repeat(MOST_LINE - 14));
        assert_eq!(long(0).len(), MOST_LINE);
        for n in 0..9 {
            events.push(Level::INFO, &long(n));
        }
        let read = all_from(&events, Some(0));
        assert_eq!((read.lines.len(), read.missed, read.next), (8, 1, 9));
        assert_eq!(&*read.lines[0], long(1));
    }

    #[test]
    fn a_read_takes_the_events_at_its_level_from_where_it_says() {
        let events = Events::new();
        let levels = [Level::INFO, Level::WARN, Level::DEBUG, Level::ERROR];
        for (n, level) in levels.into_iter().enumerate() {
            events.push(level, &line(n));
        }
        let at = |from, last, min| events.read(from, last, min, Duration::ZERO, usize::MAX);
        let warned = at(None, None, Level::WARN);
        assert_eq!(lines(&warned), [line(1), line(3)]);
        assert_eq!((warned.next, warned.end, warned.missed), (4, 4, 0));
        assert_eq!(lines(&at(None, Some(1), Level::TRACE)), [line(3)]);
        assert_eq!(
            lines(&at(None, Some(3), Level::INFO)),
            [line(0), line(1), line(3)]
        );
        assert_eq!(lines(&at(None, Some(9), Level::WARN)), [line(1), line(3)]);
        assert_eq!(lines(&at(Some(2), None, Level::TRACE)), [line(2), line(3)]);
        // Neither the last 0 nor a `from` past the end reads what's there.
        for (from, last) in [(None, Some(0)), (Some(9), None)] {
            let read = at(from, last, Level::TRACE);
            assert!(read.lines.is_empty(), "{from:?} {last:?}");
            assert_eq!((read.next, read.missed), (4, 0), "{from:?} {last:?}");
        }
        // At least one line, whatever `max_bytes`; the next read goes on from it.
        let first = events.read(None, None, Level::TRACE, Duration::ZERO, 1);
        assert_eq!((lines(&first), first.next), (vec![&*line(0)], 1));
        let rest = events.read(Some(first.next), None, Level::TRACE, Duration::ZERO, 16);
        assert_eq!((lines(&rest), rest.next), (vec![&*line(1), &*line(2)], 3));
    }

    #[test]
    fn a_read_waits_for_an_event_at_its_level_or_until_its_time_is_up() {
        let events: &'static Events = Box::leak(Box::default());
        events.push(Level::INFO, &line(0));
        let pusher = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            events.push(Level::DEBUG, &line(1));
            std::thread::sleep(Duration::from_millis(50));
            events.push(Level::ERROR, &line(2));
        });
        let start = Instant::now();
        let read = events.read(
            Some(1),
            None,
            Level::WARN,
            Duration::from_secs(20),
            usize::MAX,
        );
        pusher.join().expect("the pusher didn't panic");
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "woken by the push"
        );
        assert_eq!(lines(&read), [line(2)]);
        assert_eq!((read.next, read.missed), (3, 0));

        let start = Instant::now();
        let read = events.read(Some(3), None, Level::TRACE, Duration::from_millis(50), 1);
        assert!(start.elapsed() >= Duration::from_millis(50));
        assert!(read.lines.is_empty());
        assert_eq!((read.next, read.end), (3, 3));
    }

    #[test]
    fn lines_that_arent_json_or_are_too_long_arent_kept() {
        let events = Events::new();
        events.push(Level::ERROR, "Unable to format the following event");
        assert_eq!(all_from(&events, None).end, 0, "not numbered");
        let long = format!("{{\"x\":\"{}\"}}", "x".repeat(MOST_LINE));
        events.push(Level::ERROR, &long);
        let read = all_from(&events, Some(0));
        assert!(read.lines.is_empty());
        assert_eq!((read.next, read.end, read.missed), (1, 1, 1));
        events.push(Level::INFO, &line(2));
        let read = all_from(&events, Some(0));
        assert_eq!(lines(&read), [line(2)]);
        assert_eq!((read.next, read.missed), (2, 1));
    }

    #[test]
    fn an_event_logged_holding_the_lock_isnt_kept() {
        let events = Events::new();
        {
            let _holding = Holding::new();
            let _ring = events.lock();
            events.push(Level::ERROR, &line(0));
        }
        assert_eq!(all_from(&events, None).end, 0);
        events.push(Level::ERROR, &line(1));
        assert_eq!(lines(&all_from(&events, None)), [line(1)]);
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
