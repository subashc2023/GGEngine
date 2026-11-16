//! A game whose project's manifest doesn't read ends as its run starts, with status 1 and the
//! manifest's error last in its log, before any renderer, so it needs no GPU.
//!
//! It runs in a process of its own, since a process starts the log once.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use gg_engine::App;

#[test]
fn a_project_whose_manifest_doesnt_read_ends_the_run_as_it_starts() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bad-project");
    fs::create_dir_all(&dir).expect("a directory");
    fs::write(dir.join("project.ron"), "(\n    game: \"bad\",\n)\n").expect("written");
    let app = App::new("Bad Project").project(&dir);
    let status = gg_engine::run_with(app, ["--headless", "--frames", "1"]);
    assert_eq!(status, ExitCode::FAILURE);
    let logs = gg_engine::log_dir("Bad Project");
    let ending = format!("-{}.jsonl", std::process::id());
    let log = fs::read_dir(&logs).into_iter().flatten().flatten();
    let log = log
        .map(|entry| entry.path())
        .find(|path| path.to_str().is_some_and(|path| path.ends_with(&ending)))
        .expect("the run's log");
    let text = fs::read_to_string(&log).expect("the log");
    let last = text.lines().last().unwrap_or_default();
    // A build without `dev` reads no manifest, and runs from the game's package, which isn't there.
    let code = if cfg!(feature = "dev") {
        "project.bad"
    } else {
        "engine.no_package"
    };
    let error =
        last.contains("\"level\":\"ERROR\"") && last.contains(&format!("\"code\":\"{code}\""));
    assert!(error, "{}: {last}", log.display());
}
