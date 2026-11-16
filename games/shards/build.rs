//! Gives the game's code `GG_SOURCE`, the digest its sessions name its build by, and its Windows
//! executable version info, as gg-api's `build` says.

fn main() -> Result<(), gg_api::Failure> {
    gg_api::build::emit()
}
