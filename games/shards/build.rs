//! Tells the game's code the digest of what it's compiled from, `GG_SOURCE`, which its sessions name
//! its build by, as gg-api's `build` says.

fn main() -> Result<(), gg_api::Failure> {
    gg_api::build::emit()
}
