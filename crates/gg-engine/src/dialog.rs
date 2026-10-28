//! The OS's own file dialogs, which tools show to pick a file to open or a name to save as,
//! through rfd: Windows' common item dialog, macOS's open and save panels, and elsewhere the XDG
//! desktop portal's.

use std::path::{Path, PathBuf};

/// A dialog of the OS's own that picks a file, which
/// [`Context::pick_file`](crate::Context::pick_file) shows over the main window.
pub struct FileDialog<'a> {
    /// What its title bar says, such as "Open a scene".
    pub title: &'a str,
    /// Whether it picks a file to save as, which needn't exist yet, rather than one to open.
    pub save: bool,
    /// The files it shows: their name, such as "Scenes", and their extensions, without dots, such
    /// as `["ron"]`; every file without one. Windows' and macOS's dialogs add the first extension
    /// to a name to save as that's typed without one; the portal's needn't.
    pub filter: Option<(&'a str, &'a [&'a str])>,
    /// The directory it opens in, relative to the working directory if it's relative; without
    /// one, the OS's choice, such as where its last dialog was.
    pub directory: Option<&'a Path>,
    /// The name it suggests, such as the file the scene was last saved to.
    pub file_name: Option<&'a str>,
}

impl FileDialog<'_> {
    /// Shows the dialog over `parent`, modal, until it closes, and returns the path chosen, or
    /// `None` if it was cancelled.
    pub(crate) fn show(&self, parent: &winit::window::Window) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(self.title)
            .set_parent(parent);
        if let Some((name, extensions)) = self.filter {
            dialog = dialog.add_filter(name, extensions);
        }
        // Windows' dialog ignores one that's relative or has forward slashes, which `absolute`
        // makes backslashes there.
        if let Some(directory) = self.directory.and_then(|dir| std::path::absolute(dir).ok()) {
            dialog = dialog.set_directory(directory);
        }
        if let Some(file_name) = self.file_name {
            dialog = dialog.set_file_name(file_name);
        }
        if self.save {
            dialog.save_file()
        } else {
            dialog.pick_file()
        }
    }
}
