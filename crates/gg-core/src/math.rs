//! Vectors, matrices and quaternions, and the float functions std leaves to the platform: math
//! that gives the same bits on every CPU, as the simulation needs. The types are glam's, used
//! through this module so that every crate and game shares one version, built without SIMD and
//! with libm's functions. [`libm`] has the functions, such as `libm::sinf`, that std's float
//! methods leave to the platform's C library and clippy refuses for that (clippy.toml): a port of
//! musl's in Rust, the same code everywhere.

pub use glam::*;
pub use libm;

/// `radians` in degrees: one multiplication, the same everywhere, which `f32::to_degrees` doesn't
/// promise. By core's constant, so that it gives what that does today.
pub fn to_degrees(radians: f32) -> f32 {
    radians * 57.29578
}

/// `degrees` in radians, as [`to_degrees`] makes them degrees.
pub fn to_radians(degrees: f32) -> f32 {
    degrees * (std::f32::consts::PI / 180.0)
}
