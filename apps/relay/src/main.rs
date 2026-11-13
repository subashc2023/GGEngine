//! The relay, `gg-relay`, that a session's runs meet through when they can't connect to each
//! other: `gg-relay --help` says how it's run, and gg-relay's root what it does.

use std::process::ExitCode;

fn main() -> ExitCode {
    gg_relay::run(std::env::args_os().skip(1))
}
