//! Math that gives the same bits on every CPU, as the simulation needs.
//!
//! The types are glam's, built without SIMD and with libm's functions, used through here so every
//! crate and game shares one version. [`libm`], a Rust port of musl's, has the float functions,
//! such as `libm::sinf`, that std leaves to the platform's C library and clippy.toml bans.

pub use glam::*;
pub use libm;

/// `radians` in degrees by one multiplication everywhere, which `f32::to_degrees` doesn't promise.
/// The constant is core's, so it gives what that does today.
pub fn to_degrees(radians: f32) -> f32 {
    radians * 57.29578
}

/// `degrees` in radians, by one multiplication as [`to_degrees`] is.
pub fn to_radians(degrees: f32) -> f32 {
    degrees * (std::f32::consts::PI / 180.0)
}
