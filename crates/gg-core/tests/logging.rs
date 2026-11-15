//! A panic reaches the console and the JSON log under each filter tried, `off` among them; a
//! filter that names targets alone keeps the default for the rest; a value that isn't a filter
//! keeps the default, warning with `log.filter`; the events kept in memory are the JSON log's
//! lines, as written; and without the console, nothing logged reaches stderr. Logging and the panic
//! hook are the process's, so for each filter the test runs itself again, as a run that logs and
//! then panics, and reads what that run printed and wrote.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use tracing::Level;

/// Set in the runs the test makes of itself: where they log.
const RUN: &str = "GG_LOGGING_TEST_DIR";

/// Set in a run the test makes of itself that logs without the console.
const QUIET: &str = "GG_LOGGING_TEST_QUIET";

/// The events a run logs before it panics: a game's at info, and one of gg-gpu's at trace.
const EVENTS: [&str; 2] = ["a game's event", "a gg-gpu trace event"];

fn main() {
    if let Some(dir) = std::env::var_os(RUN) {
        log_and_panic(Path::new(&dir));
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("logs");
    // Each filter, the events that pass it, whether it warns that it isn't a filter, and whether
    // the run logs on the console.
    let filters: [(&str, &[&str], bool, bool); 5] = [
        ("gg_gpu=trace", &EVENTS, false, true),
        ("info,gg_gpu=debug", &EVENTS[..1], false, true),
        ("off", &[], false, true),
        // `loud` is no level, so the default applies.
        ("gg_gpu=loud", &EVENTS[..1], true, true),
        ("gg_gpu=trace", &EVENTS, false, false),
    ];
    for (filter, shown, warns, console) in filters {
        let mut run = Command::new(std::env::current_exe().expect("the test's path"));
        run.env(RUN, &dir)
            .env("RUST_LOG", filter)
            .env("NO_COLOR", "1")
            .env_remove(QUIET);
        if !console {
            run.env(QUIET, "1");
        }
        let run = run.output().expect("a run of the test");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(101), "{filter}: {stderr}");
        assert_eq!(
            stderr.contains("[log.panic] the run panics"),
            console,
            "{filter}: the panic on the console:\n{stderr}"
        );
        let stdout = String::from_utf8(run.stdout).expect("what the run printed");
        let mut printed = stdout.lines();
        let path = printed.next().expect("the JSON log's path");
        let json = std::fs::read_to_string(path.trim()).expect("the JSON log");
        let panic = json
            .lines()
            .find(|line| line.contains(r#""code":"log.panic""#));
        assert!(
            panic.is_some_and(|line| line.contains("the run panics")),
            "{filter}: the JSON log has no panic:\n{json}"
        );
        check_frames(panic.unwrap_or_default());
        // The run printed the events kept before it panicked.
        let kept: Vec<&str> = printed.collect();
        let written: Vec<&str> = (json.lines())
            .filter(|line| !line.contains(r#""code":"log.panic""#))
            .collect();
        assert_eq!(kept, written, "{filter}: the events kept");
        let warning = json
            .lines()
            .find(|line| line.contains(r#""code":"log.filter""#));
        assert_eq!(
            warning.is_some_and(|line| {
                line.contains(r#""level":"WARN""#)
                    && line.contains(&format!(r#""value":"{filter}""#))
            }),
            warns,
            "{filter}: log.filter in the JSON log:\n{json}"
        );
        assert_eq!(
            stderr.contains("[log.filter] "),
            warns && console,
            "{filter}: log.filter on the console:\n{stderr}"
        );
        for event in EVENTS {
            let expected = shown.contains(&event);
            assert_eq!(
                stderr.contains(event),
                expected && console,
                "{filter}: {event}\n{stderr}"
            );
            assert_eq!(json.contains(event), expected, "{filter}: {event}\n{json}");
        }
        let without = if console { "" } else { " without the console" };
        println!("test a panic is logged under RUST_LOG={filter}{without} ... ok");
    }
}

/// Checks the frames the panic's line, `panic`, names, innermost first, and its backtrace, logged
/// whatever `RUST_BACKTRACE` says: the first frame is in the test's own executable, which the
/// panic hook is linked into, by its file name, at an offset within the file.
fn check_frames(panic: &str) {
    let field = |name: &str| {
        let rest = panic.split(&format!(r#""{name}":""#)).nth(1)?;
        rest.split('"').next()
    };
    assert!(
        field("backtrace").is_some_and(|backtrace| !backtrace.is_empty()),
        "{panic}"
    );
    let frames = field("frames").expect("the panic's frames");
    let exe = std::env::current_exe().expect("the test's path");
    let name = exe
        .file_name()
        .and_then(|name| name.to_str())
        .expect("its name");
    let first = frames.split(' ').next().expect("a frame");
    let (module, offset) = first.split_once("+0x").expect("a module and an offset");
    let offset = u64::from_str_radix(offset, 16).expect("hex digits");
    let size = std::fs::metadata(&exe).expect("the test's file").len();
    assert_eq!(module, name, "{frames}");
    assert!(offset < size, "{offset:#x} of {size} bytes: {frames}");
    assert!(frames.split(' ').count() > 3, "{frames}");
}

/// Starts logging, on the console unless [`QUIET`] is set, prints the JSON log's path, logs
/// [`EVENTS`], prints the lines of the events kept, and panics.
fn log_and_panic(dir: &Path) -> ! {
    let console = std::env::var_os(QUIET).is_none();
    let log = gg_core::log::init("logging test", dir, console).expect("the JSON log");
    println!("{}", log.display());
    tracing::info!(target: "game", "{}", EVENTS[0]);
    tracing::trace!(target: "gg_gpu::test", "{}", EVENTS[1]);
    let kept = gg_core::log::EVENTS.read(None, None, Level::TRACE, Duration::ZERO, usize::MAX);
    for line in kept.lines {
        println!("{line}");
    }
    panic!("the run panics");
}
