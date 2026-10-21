//! Vectors, matrices and quaternions. They're glam's, used through this module so that every
//! crate and game shares one version, and so that the deterministic mode, which needs math that
//! is identical on every CPU, can change what's behind these names in one place.

pub use glam::*;
