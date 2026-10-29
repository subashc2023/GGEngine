//! The layout's files: where they live, the app's own and those the user saves by name, and
//! reading and writing them as RON.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::{fs, io};

use gg_core::{Error, Result};

use super::tree::Layout;
use crate::dirs::{self, Kind};

/// The most characters a layout's name has.
const NAME: usize = 64;

/// The names Windows keeps for devices, whatever their case: a file of one is the device. Its
/// superscript digits count as digits.
const DEVICES: [&str; 30] = [
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

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
    Ok(read(path, view)?.unwrap_or_else(|| Layout::new(view)))
}

/// Reads the layout saved as `name` beside the app's layout file at `path`, as [`load`] reads that
/// one, but for an error if there's none.
pub(crate) fn load_named(path: &Path, name: &str, view: Option<&'static str>) -> Result<Layout> {
    let path = named(path, name);
    read(&path, view)?.ok_or_else(|| {
        let message = format!("there's no saved layout {}", path.display());
        Error::new("engine.dock_layout", message)
    })
}

/// What [`load`] reads: `None` if there's no file. A view's panel closed opens, since nothing else
/// shows the game.
fn read(path: &Path, view: Option<&'static str>) -> Result<Option<Layout>> {
    let message = format!("can't read the dock's layout from {}", path.display());
    let bytes = match dirs::read_file(path, u64::MAX) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::new("engine.dock_layout", message).with_source(e)),
    };
    let error = |problem: String| Error::new("engine.dock_layout", format!("{message}: {problem}"));
    let text = String::from_utf8(bytes).map_err(|e| error(e.to_string()))?;
    let mut layout: Layout = ron::from_str(&text).map_err(|e| error(e.to_string()))?;
    layout.view = view;
    layout.check().map_err(error)?;
    layout.closed.retain(|panel| Some(panel.as_str()) != view);
    layout.prune();
    layout.renumber();
    Ok(Some(layout))
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

/// Saves `layout` as `name` beside the app's layout file at `path`, as [`save`] does: in place of
/// the layout saved under that name, whatever its case, whose name it keeps, as Windows' and
/// macOS's file systems keep a file's.
pub(crate) fn save_named(path: &Path, name: &str, layout: &Layout) -> Result<()> {
    let saved = names(path)?.into_iter().find(|saved| same(saved, name));
    save(&named(path, saved.as_deref().unwrap_or(name)), layout)
}

/// Deletes the layout saved as `name` beside the app's layout file at `path`.
pub(crate) fn delete_named(path: &Path, name: &str) -> Result<()> {
    let path = named(path, name);
    fs::remove_file(&path).map_err(|e| {
        let message = format!("can't delete the saved layout {}", path.display());
        Error::new("engine.dock_layout", message).with_source(e)
    })
}

/// The names of the layouts saved beside the app's layout file at `path`, alphabetically: none
/// before one is.
pub(crate) fn names(path: &Path) -> Result<Vec<String>> {
    let dir = layouts(path);
    let error = |e: io::Error| {
        let message = format!("can't list the saved layouts in {}", dir.display());
        Error::new("engine.dock_layout", message).with_source(e)
    };
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(error(e)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(error)?;
        let path = entry.path();
        // The files the dock saves, whose names a layout's can be, rather than one being written.
        if entry.file_type().map_err(error)?.is_file()
            && path.extension() == Some(OsStr::new("ron"))
            && let Some(stem) = path.file_stem().and_then(OsStr::to_str)
            && name(stem).is_ok_and(|name| name == stem)
        {
            names.push(stem.to_owned());
        }
    }
    names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    Ok(names)
}

/// The name `typed` for a layout, trimmed: 1 to 64 letters, digits, spaces, `-` and `_`, and not
/// one Windows keeps for a device, on any OS, since layouts move between machines. An
/// `engine.bad_layout_name` error says why not.
pub(crate) fn name(typed: &str) -> Result<&str> {
    let name = typed.trim();
    let refused = |why: String| Err(Error::new("engine.bad_layout_name", why));
    let length = name.chars().count();
    if length == 0 {
        return refused("a layout's name can't be empty".to_owned());
    }
    if length > NAME {
        return refused(format!(
            "a layout's name is {NAME} characters at most, not {length}"
        ));
    }
    let fits = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_');
    if let Some(c) = name.chars().find(|&c| !fits(c)) {
        return refused(format!(
            "a layout's name can't have {c:?}, only letters, digits, spaces, '-' and '_'"
        ));
    }
    if DEVICES
        .iter()
        .any(|device| device.eq_ignore_ascii_case(name))
    {
        return refused(format!(
            "a layout can't be named {name:?}, which Windows keeps for a device"
        ));
    }
    Ok(name)
}

/// Whether two layouts' names are the same name, without case, as Windows' and macOS's file
/// systems compare them.
fn same(a: &str, b: &str) -> bool {
    let a = a.chars().flat_map(char::to_lowercase);
    a.eq(b.chars().flat_map(char::to_lowercase))
}

/// The directory of the layouts saved by name, beside the app's layout file at `path`.
fn layouts(path: &Path) -> PathBuf {
    path.with_file_name("layouts")
}

/// The file of the layout saved as `name`, beside the app's layout file at `path`.
fn named(path: &Path, name: &str) -> PathBuf {
    layouts(path).join(format!("{name}.ron"))
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

    #[test]
    fn closed_panels_save_and_load_and_older_files_and_the_views_never_close() {
        let dir = Scratch::new("dock-closed");
        let file = dir.0.join("layout.ron");
        let view = Some("View");
        let mut layout = Layout::new(view);
        let place = Place::Main {
            min: [10.0, 20.0],
            size: [300.0, 200.0],
        };
        layout.float("Stats".to_owned(), place);
        assert!(layout.close("Stats"));
        save(&file, &layout).expect("the layout is saved");
        let loaded = load(&file, view).expect("the layout loads");
        assert_eq!(loaded.closed, ["Stats"]);
        // A file from before panels closed, and one that closes the view, which shows anyway.
        for (closed, loads) in [
            ("", vec![]),
            (r#"closed: ["View", "Stats"],"#, vec!["Stats"]),
        ] {
            let text = format!(
                "(main: Tabs((panels: [\"View\"], active: 0)), floating: [(tabs: (panels: \
                 [\"Stats\"], active: 0), place: Main(min: (1.0, 2.0), size: (3.0, 4.0)))], \
                 {closed})"
            );
            fs::write(&file, text).expect("the file is written");
            let loaded = load(&file, view).expect("the layout loads");
            assert_eq!(loaded.closed, loads, "{closed}");
        }
    }

    #[test]
    fn layouts_save_by_name_and_list_load_and_delete() {
        let dir = Scratch::new("dock-named");
        let file = dir.0.join("layout.ron");
        assert_eq!(names(&file).ok(), Some(Vec::new()));
        let mut wide = Layout::new(None);
        let a = wide.float(
            "A".to_owned(),
            Place::Main {
                min: [10.0, 20.0],
                size: [300.0, 200.0],
            },
        );
        assert!(wide.dock(a, CENTRAL, Zone::Side(Side::Left)));
        assert!(wide.close("A"));
        for name in ["wide", "Tall", "narrow"] {
            save_named(&file, name, &wide).expect("the layout is saved");
        }
        // Beside the app's own, which isn't one of them, nor is a file being written.
        save(&file, &Layout::new(None)).expect("the layout is saved");
        fs::write(dir.0.join("layouts/half.ron.new"), "").expect("the file is written");
        let listed = |names: &[&str]| Some(names.iter().map(|&name| name.to_owned()).collect());
        assert_eq!(names(&file).ok(), listed(&["narrow", "Tall", "wide"]));
        let mut loaded = load_named(&file, "wide", None).expect("the layout loads");
        loaded.renumber();
        wide.renumber();
        assert_eq!(loaded, wide);
        // Saved under a name that's taken, whatever its case, it replaces that layout.
        save_named(&file, "WIDE", &Layout::new(None)).expect("the layout is saved");
        assert_eq!(names(&file).ok(), listed(&["narrow", "Tall", "wide"]));
        let replaced = load_named(&file, "wide", None).expect("the layout loads");
        assert_eq!(replaced, Layout::new(None));
        delete_named(&file, "Tall").expect("the layout is deleted");
        assert_eq!(names(&file).ok(), listed(&["narrow", "wide"]));
        let error = load_named(&file, "Tall", None).expect_err("there's no such layout");
        assert_eq!(error.code(), "engine.dock_layout");
        assert!(delete_named(&file, "Tall").is_err());
    }

    #[test]
    fn a_layouts_name_is_trimmed_and_one_that_cant_be_says_why() {
        assert_eq!(name("  My layout_2-b ").ok(), Some("My layout_2-b"));
        assert_eq!(name("Ünïcödé 名前").ok(), Some("Ünïcödé 名前"));
        assert_eq!(name(&"x".repeat(64)).ok().map(str::len), Some(64));
        assert!(name("com10").is_ok());
        for (typed, why) in [
            ("   ", "can't be empty"),
            (&"x".repeat(65), "64 characters at most, not 65"),
            ("a/b", "can't have '/'"),
            ("a.ron", "can't have '.'"),
            ("con", "Windows keeps for a device"),
            ("Com1", "Windows keeps for a device"),
            (" LPT9 ", "Windows keeps for a device"),
            ("nul", "Windows keeps for a device"),
            ("com²", "Windows keeps for a device"),
        ] {
            let error = name(typed).expect_err(typed);
            assert_eq!(error.code(), "engine.bad_layout_name");
            assert!(error.to_string().contains(why), "{typed:?}: {error}");
        }
    }
}
