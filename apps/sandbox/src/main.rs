//! Development playground: test scenes and debug tooling for engine work.

use std::process::ExitCode;

fn main() -> ExitCode {
    gg_engine::run(gg_engine::App::new("Sandbox"))
}
