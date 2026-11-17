//! Docking: panels layers show.
//!
//! Users arrange them in tabs and splits around the central area, tear them into windows, close and
//! reopen them, and save layouts by name. Only `dev` builds have it; others have `none.rs`.

mod docked;
mod file;
mod tree;
mod ui;

pub use docked::Docked;
pub(crate) use file::path;
pub use ui::Ui;
pub(crate) use ui::{Dock, Under};
