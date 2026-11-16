//! Math, logging, profiling and errors: what the other `gg-*` crates but `gg-macros` build on.
//!
//! It needs no other `gg-*` crate and nothing GPU- or window-specific, so any crate can use it.

mod error;
pub mod log;
pub mod math;
pub mod profile;

pub use error::{Error, Result};
