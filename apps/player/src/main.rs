//! Shipping runtime, to run a cooked project without development tooling; for now it runs the
//! engine with no layers.

use std::process::ExitCode;

fn main() -> ExitCode {
    gg_engine::run(gg_engine::App::new("Player"))
}
