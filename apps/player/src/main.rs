//! Shipping runtime: runs a cooked project, without development tooling.

use std::process::ExitCode;

fn main() -> ExitCode {
    gg_engine::run(gg_engine::App::new("Player"))
}
