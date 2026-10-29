//! A panic reaches the console and the JSON log under each filter tried, `off` among them; a
//! filter that names targets alone keeps the default for the rest; and a value that isn't a filter
//! keeps the default, warning with `log.filter`. Logging and the panic hook are the process's, so
//! for each filter the test runs itself again, as a run that logs and then panics, and reads what
//! that run printed and wrote.

use std::path::Path;
use std::process::Command;

/// Set in the runs the test makes of itself: where they log.
const RUN: &str = "GG_LOGGING_TEST_DIR";

/// The events a run logs before it panics: a game's at info, and one of gg-gpu's at trace.
const EVENTS: [&str; 2] = ["a game's event", "a gg-gpu trace event"];

fn main() {
    if let Some(dir) = std::env::var_os(RUN) {
        log_and_panic(Path::new(&dir));
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("logs");
    // Each filter, the events that pass it, and whether it warns that it isn't a filter.
    let filters: [(&str, &[&str], bool); 4] = [
        ("gg_gpu=trace", &EVENTS, false),
        ("info,gg_gpu=debug", &EVENTS[..1], false),
        ("off", &[], false),
        // `loud` is no level, so the default applies.
        ("gg_gpu=loud", &EVENTS[..1], true),
    ];
    for (filter, shown, warns) in filters {
        let run = Command::new(std::env::current_exe().expect("the test's path"))
            .env(RUN, &dir)
            .env("RUST_LOG", filter)
            .env("NO_COLOR", "1")
            .output()
            .expect("a run of the test");
        let console = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(101), "{filter}: {console}");
        assert!(
            console.contains("[log.panic] the run panics"),
            "{filter}: the console has no panic:\n{console}"
        );
        let path = String::from_utf8(run.stdout).expect("the JSON log's path");
        let json = std::fs::read_to_string(path.trim()).expect("the JSON log");
        let panic = json
            .lines()
            .find(|line| line.contains(r#""code":"log.panic""#));
        assert!(
            panic.is_some_and(|line| line.contains("the run panics")),
            "{filter}: the JSON log has no panic:\n{json}"
        );
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
            console.contains("[log.filter] "),
            warns,
            "{filter}: log.filter on the console:\n{console}"
        );
        for event in EVENTS {
            let expected = shown.contains(&event);
            assert_eq!(
                console.contains(event),
                expected,
                "{filter}: {event}\n{console}"
            );
            assert_eq!(json.contains(event), expected, "{filter}: {event}\n{json}");
        }
        println!("test a panic is logged under RUST_LOG={filter} ... ok");
    }
}

/// Starts logging, prints the JSON log's path, logs [`EVENTS`] and panics.
fn log_and_panic(dir: &Path) -> ! {
    let log = gg_core::log::init("logging test", dir).expect("the JSON log");
    println!("{}", log.display());
    tracing::info!(target: "game", "{}", EVENTS[0]);
    tracing::trace!(target: "gg_gpu::test", "{}", EVENTS[1]);
    panic!("the run panics");
}
