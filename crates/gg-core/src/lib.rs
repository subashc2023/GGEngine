//! Foundation for every other `gg-*` crate: math, handles, logging, timing and error types.
//!
//! It depends on no other `gg-*` crate and on nothing GPU- or window-specific, so any crate
//! can depend on it.

mod error;
pub mod log;

pub use error::{Error, Result};
