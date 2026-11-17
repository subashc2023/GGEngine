//! A layer whose load fails ends the run as it starts, before any layer attaches, with status 1 and
//! the load's error last in its log. It needs a GPU, as the headless renderer starts first, and
//! runs in a process of its own, since a process starts the log once.

use std::fs;
use std::process::ExitCode;

use gg_engine::{App, Context, Error, Layer};

/// Fails to load; never attaches.
struct Unloadable;

impl Layer for Unloadable {
    fn load(&mut self, _: &mut Context) -> Result<(), Error> {
        Err(Error::new("test.unloadable", "the layer can't load"))
    }

    fn attach(&mut self, _: &mut Context) {
        panic!("a layer attached after a load failed");
    }
}

#[test]
fn a_layer_whose_load_fails_ends_the_run_as_it_starts() {
    if std::env::var_os("GG_SKIP_GPU_TESTS").is_some_and(|value| !value.is_empty()) {
        println!("skipped: GG_SKIP_GPU_TESTS is set");
        return;
    }
    let app = App::new("Unloadable").layer(Unloadable);
    let status = gg_engine::run_with(app, ["--headless", "--frames", "1"]);
    assert_eq!(status, ExitCode::FAILURE);
    // Other apps' logs share the folder, and a PID can be an earlier run's.
    let ending = format!("-{}.jsonl", std::process::id());
    let log = fs::read_dir(gg_engine::log_dir("Unloadable"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            name.starts_with("unloadable-") && name.ends_with(&ending)
        })
        .max()
        .expect("the run's log");
    let text = fs::read_to_string(&log).expect("the log");
    let last = text.lines().last().unwrap_or_default();
    let error =
        last.contains("\"level\":\"ERROR\"") && last.contains("\"code\":\"test.unloadable\"");
    assert!(error, "{}: {last}", log.display());
}
