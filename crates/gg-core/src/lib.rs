//! Foundation for every other `gg-*` crate: its math, logging, profiling and error types, and the
//! handles to come.
//!
//! It depends on no other `gg-*` crate and on nothing GPU- or window-specific, so any crate
//! can depend on it.

mod error;
pub mod log;
pub mod math;
pub mod profile;

pub use error::{Error, Result};
