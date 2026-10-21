//! Foundation for every other `gg-*` crate: its math, logging and error types, and the handles
//! and timing to come.
//!
//! It depends on no other `gg-*` crate and on nothing GPU- or window-specific, so any crate
//! can depend on it.

mod error;
pub mod log;
pub mod math;

pub use error::{Error, Result};
