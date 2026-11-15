//! Refuses a build whose C++ flags would let Luau, which mlua builds, fuse a script's
//! multiplications into additions, as gg-api's `build::check_cxx_contract` says: the engine's
//! `.cargo/config.toml` sets them, and a game's workspace sets them in its own.

use std::env;

fn main() -> Result<(), gg_api::Failure> {
    // It reads no file: without this, cargo would run it again as any of the package's changes.
    println!("cargo::rerun-if-changed=build.rs");
    let cargo = |name: &str| {
        env::var(name).map_err(|_| {
            let message = format!("cargo set no `{name}`, as it does for a build script");
            gg_api::Failure::new("build.metadata", message)
        })
    };
    let var = |name: &str| {
        println!("cargo::rerun-if-env-changed={name}");
        env::var_os(name).map(|value| value.to_string_lossy().into_owned())
    };
    gg_api::build::check_cxx_contract(&cargo("TARGET")?, &cargo("HOST")?, var)
}
