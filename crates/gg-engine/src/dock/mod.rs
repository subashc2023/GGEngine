//! Docking: panels that layers show, which the user arranges in tabs and splits around the
//! central area, or tears off into windows of their own.

mod file;
mod tree;
mod ui;

pub(crate) use file::path;
pub(crate) use ui::Dock;
pub use ui::Ui;
