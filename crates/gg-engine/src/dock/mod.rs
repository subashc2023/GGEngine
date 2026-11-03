//! Docking: panels that layers show, which the user arranges in tabs and splits around the
//! central area, tears off into windows of their own, and closes and opens again, and the layouts
//! they make, which the user saves and loads by name. In development builds alone, as `dev` has
//! the UI: a build without it has `none.rs` in its place.

mod docked;
mod file;
mod tree;
mod ui;

pub use docked::Docked;
pub(crate) use file::path;
pub use ui::Ui;
pub(crate) use ui::{Dock, Under};
