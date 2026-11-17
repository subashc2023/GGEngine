//! An app that runs from a game's package, as the reference player does, ends as its run starts
//! without one, in any build, with status 1 and `engine.no_package` last in its log, before any
//! renderer, so it needs no GPU.
//!
//! It runs in a process of its own, since a process starts the log once.

use std::fs;
use std::process::ExitCode;

use gg_engine::App;

#[test]
fn an_app_that_runs_from_a_package_without_one_ends_as_it_starts() {
    let app = App::new("No Package").from_package();
    let status = gg_engine::run_with(app, ["--headless", "--frames", "1"]);
    assert_eq!(status, ExitCode::FAILURE);
    // Other apps' logs share the folder, and a PID can be an earlier run's.
    let ending = format!("-{}.jsonl", std::process::id());
    let log = fs::read_dir(gg_engine::log_dir("No Package"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            name.starts_with("no-package-") && name.ends_with(&ending)
        })
        .max()
        .expect("the run's log");
    let text = fs::read_to_string(&log).expect("the log");
    let last = text.lines().last().unwrap_or_default();
    let error =
        last.contains("\"level\":\"ERROR\"") && last.contains("\"code\":\"engine.no_package\"");
    assert!(error, "{}: {last}", log.display());
}
