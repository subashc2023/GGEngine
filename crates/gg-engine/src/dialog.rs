//! The OS's own file dialogs, through rfd: Windows' common item dialog, macOS's open and save
//! panels, elsewhere the XDG desktop portal's.

use std::path::{Path, PathBuf};

/// An OS file or folder dialog that [`Context::pick_file`](crate::Context::pick_file) shows over
/// the main window.
pub struct FileDialog<'a> {
    /// Such as "Open a scene".
    pub title: &'a str,
    pub picks: Picks,
    /// A name, such as "Scenes", and extensions without dots, such as `["ron"]`; without one,
    /// every file shows. Windows' and macOS's dialogs add the first extension to a typed save name
    /// lacking one; the portal's needn't. A folder's dialog ignores it.
    pub filter: Option<(&'a str, &'a [&'a str])>,
    /// Where it opens, relative to the working directory; without one, the OS's choice.
    pub directory: Option<&'a Path>,
    /// The name it suggests, such as the scene's last file. A folder's dialog ignores it.
    pub file_name: Option<&'a str>,
}

/// What a [`FileDialog`] picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picks {
    /// An existing file to open.
    Open,
    /// A file to save as; it needn't exist.
    Save,
    /// An existing folder, such as one to make a project in.
    Folder,
}

impl FileDialog<'_> {
    /// Shows the dialog over `parent`, modal. `None` if cancelled.
    pub(crate) fn show(&self, parent: &winit::window::Window) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(self.title)
            .set_parent(parent);
        if let Some((name, extensions)) = self.filter {
            dialog = dialog.add_filter(name, extensions);
        }
        // Windows' dialog ignores a relative directory or one with forward slashes; `absolute`
        // fixes both.
        if let Some(directory) = self.directory.and_then(|dir| std::path::absolute(dir).ok()) {
            dialog = dialog.set_directory(directory);
        }
        if let Some(file_name) = self.file_name {
            dialog = dialog.set_file_name(file_name);
        }
        match self.picks {
            Picks::Open => dialog.pick_file(),
            Picks::Save => dialog.save_file(),
            Picks::Folder => dialog.pick_folder(),
        }
    }
}
