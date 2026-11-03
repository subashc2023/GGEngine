//! An app that runs from a game's package, as the reference player does, ends as its run starts
//! without one, in any build, with status 1 and `engine.no_package` as the last thing its log says,
//! before any renderer, so that it needs no GPU.
//!
//! It runs in a process of its own, as a run starts the log, which a process starts once.

use std::fs;
use std::process::ExitCode;

use gg_engine::App;

#[test]
fn an_app_that_runs_from_a_package_without_one_ends_as_it_starts() {
    let app = App::new("No Package").from_package();
    let status = gg_engine::run_with(app, ["--headless", "--frames", "1"]);
    assert_eq!(status, ExitCode::FAILURE);
    let ending = format!("-{}.jsonl", std::process::id());
    let log = fs::read_dir(gg_engine::log_dir("No Package"))
        .into_iter()
        .flatten()
        .flatten();
    let log = log
        .map(|entry| entry.path())
        .find(|path| path.to_str().is_some_and(|path| path.ends_with(&ending)))
        .expect("the run's log");
    let text = fs::read_to_string(&log).expect("the log");
    let last = text.lines().last().unwrap_or_default();
    let error =
        last.contains("\"level\":\"ERROR\"") && last.contains("\"code\":\"engine.no_package\"");
    assert!(error, "{}: {last}", log.display());
}
