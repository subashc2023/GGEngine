//! The layout file: where it lives, and reading and writing it as RON.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{fs, io};

use gg_core::{Error, Result};

use super::tree::Layout;
use crate::dirs::{self, Kind};

/// The file an app's layout is saved in, in the OS's directory for per-user state, which
/// [`dirs::dir`] says: `os` is [`std::env::consts::OS`], and `var` reads the environment. `None`
/// without the variables.
pub(crate) fn path(app: &str, os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    Some(dirs::dir(app, Kind::State, os, var)?.join("layout.ron"))
}

/// Reads the layout saved at `path`, of a dockspace whose view panel `view` names if there's one:
/// a new layout if there's no file, and an `engine.dock_layout` error if it can't be read or
/// doesn't hold a layout of that kind.
pub(crate) fn load(path: &Path, view: Option<&'static str>) -> Result<Layout> {
    let message = format!("can't read the dock's layout from {}", path.display());
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Layout::new(view)),
        Err(e) => return Err(Error::new("engine.dock_layout", message).with_source(e)),
    };
    let error = |problem: String| Error::new("engine.dock_layout", format!("{message}: {problem}"));
    let mut layout: Layout = ron::from_str(&text).map_err(|e| error(e.to_string()))?;
    layout.view = view;
    layout.check().map_err(error)?;
    layout.prune();
    layout.renumber();
    Ok(layout)
}

/// Writes `layout` to `path`, as [`dirs::write`] does, so that a crash while writing leaves the
/// last layout whole.
pub(crate) fn save(path: &Path, layout: &Layout) -> Result<()> {
    let error = |e: io::Error| {
        let message = format!("can't save the dock's layout to {}", path.display());
        Error::new("engine.dock_layout", message).with_source(e)
    };
    let text = ron::ser::to_string_pretty(layout, ron::ser::PrettyConfig::default())
        .map_err(|e| error(io::Error::other(e)))?;
    dirs::write(path, text.as_bytes()).map_err(error)
}

#[cfg(test)]
mod tests {
    use super::super::tree::{CENTRAL, Place, Side, Zone};
    use super::*;
    use crate::dirs::tests::Scratch;

    #[test]
    fn the_file_is_in_the_os_directory_for_per_user_state() {
        let home = |name: &str| (name == "HOME").then(|| OsString::from("/home/ada"));
        assert_eq!(
            path("Sandbox", "linux", home),
            Some(Path::new("/home/ada/.local/state/ggengine/Sandbox/layout.ron").to_owned())
        );
    }

    #[test]
    fn a_saved_layout_loads_as_it_was() {
        let mut layout = Layout::new(None);
        let a = layout.float(
            "Stats".to_owned(),
            Place::Main {
                min: [10.0, 20.0],
                size: [300.0, 200.0],
            },
        );
        assert!(layout.dock(a, CENTRAL, Zone::Side(Side::Left)));
        layout.float(
            "Clear color".to_owned(),
            Place::Window {
                position: [-1200, 300],
                size: [320.5, 240.0],
            },
        );
        let dir = Scratch::new("dock-round-trip");
        let file = dir.0.join("layout.ron");
        save(&file, &layout).expect("the layout is saved");
        let mut loaded = load(&file, None).expect("the layout loads");
        layout.renumber();
        loaded.renumber();
        assert_eq!(loaded, layout);
        assert!(!file.with_extension("ron.new").exists());
    }

    #[test]
    fn a_dockspace_starts_with_its_view_and_loads_as_it_was() {
        let dir = Scratch::new("dock-dockspace");
        let file = dir.0.join("layout.ron");
        let view = Some("View");
        assert_eq!(load(&file, view).ok(), Some(Layout::new(view)));
        let mut layout = Layout::new(view);
        let place = Place::Main {
            min: [10.0, 20.0],
            size: [300.0, 200.0],
        };
        let a = layout.float("Stats".to_owned(), place);
        let group = layout.find("View").expect("the view's group");
        assert!(layout.dock(a, group, Zone::Side(Side::Bottom)));
        save(&file, &layout).expect("the layout is saved");
        let mut loaded = load(&file, view).expect("the layout loads");
        layout.renumber();
        loaded.renumber();
        assert_eq!(loaded, layout);
        // It has no central area, which a game's layout needs.
        let error = load(&file, None).expect_err("a game's dock can't use it");
        assert_eq!(error.code(), "engine.dock_layout");
    }

    #[test]
    fn a_missing_file_is_the_empty_layout_and_a_broken_one_an_error() {
        let dir = Scratch::new("dock-broken");
        let file = dir.0.join("layout.ron");
        assert_eq!(load(&file, None).ok(), Some(Layout::new(None)));
        fs::create_dir_all(&dir.0).expect("a directory");
        // Text that isn't RON, and RON that isn't a layout the dock can use.
        for text in [
            "not a layout",
            "(main: Split(axis: Horizontal, ratio: 0.5, children: (Central, Central)), floating: [])",
        ] {
            fs::write(&file, text).expect("the file is written");
            let error = load(&file, None).expect_err("the file holds no usable layout");
            assert_eq!(error.code(), "engine.dock_layout");
            assert!(error.to_string().contains("layout.ron"), "{error}");
        }
    }
}
