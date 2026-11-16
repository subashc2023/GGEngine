//! The relay, `gg-relay`, that a session's runs meet through when they can't connect directly. How
//! to run it: `gg-relay --help`. What it does: gg-relay's root.

use std::process::ExitCode;

fn main() -> ExitCode {
    gg_relay::run(std::env::args_os().skip(1))
}
