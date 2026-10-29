//! Foundation for the other `gg-*` crates but `gg-macros`: their math, logging, profiling and error
//! types.
//!
//! It depends on no other `gg-*` crate and on nothing GPU- or window-specific, so any crate
//! can depend on it.

mod error;
pub mod log;
pub mod math;
pub mod profile;

pub use error::{Error, Result};
