//! Docking: panels that layers show, which the user arranges in tabs and splits around the
//! central area, tears off into windows of their own, and closes and opens again, and the layouts
//! they make, which the user saves and loads by name.

mod file;
mod tree;
mod ui;

pub(crate) use file::path;
pub use tree::Docked;
pub use ui::Ui;
pub(crate) use ui::{Dock, Under};
