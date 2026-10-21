//! The layout file: where it lives, and reading and writing it as RON.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{fs, io};

use gg_core::{Error, Result};

use super::tree::Layout;

/// The file an app's layout is saved in, in the OS's directory for per-user state, never beside
/// the executable: `%LOCALAPPDATA%\GGEngine\<app>` on Windows,
/// `~/Library/Application Support/GGEngine/<app>` on macOS, and elsewhere
/// `$XDG_STATE_HOME/ggengine/<app>`, by default `~/.local/state/ggengine/<app>`. `os` is
/// [`std::env::consts::OS`], and `var` reads the environment. `None` without the variables.
pub(crate) fn path(app: &str, os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let var = |name| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let dir = match os {
        "windows" => var("LOCALAPPDATA")?.join("GGEngine"),
        "macos" => var("HOME")?.join("Library/Application Support/GGEngine"),
        // The XDG spec says to ignore a relative path.
        _ => var("XDG_STATE_HOME")
            .filter(|dir| dir.as_os_str().as_encoded_bytes().starts_with(b"/"))
            .or_else(|| Some(var("HOME")?.join(".local/state")))?
            .join("ggengine"),
    };
    Some(dir.join(app).join("layout.ron"))
}

/// Reads the layout saved at `path`: the empty layout if there's no file, and an
/// `engine.dock_layout` error if it can't be read or doesn't hold a layout.
pub(crate) fn load(path: &Path) -> Result<Layout> {
    let error = |message: String| {
        Error::new(
            "engine.dock_layout",
            format!(
                "can't read the dock's layout from {}: {message}",
                path.display()
            ),
        )
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Layout::new()),
        Err(e) => return Err(error(e.to_string()).with_source(e)),
    };
    let mut layout: Layout = ron::from_str(&text).map_err(|e| error(e.to_string()))?;
    layout.check().map_err(error)?;
    layout.prune();
    layout.renumber();
    Ok(layout)
}

/// Writes `layout` to `path`, through a file beside it that then replaces it, so that a crash
/// while writing leaves the last layout whole.
pub(crate) fn save(path: &Path, layout: &Layout) -> Result<()> {
    let error = |e: io::Error| {
        Error::new(
            "engine.dock_layout",
            format!("can't save the dock's layout to {}: {e}", path.display()),
        )
        .with_source(e)
    };
    let text = ron::ser::to_string_pretty(layout, ron::ser::PrettyConfig::default())
        .map_err(|e| error(io::Error::other(e)))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(error)?;
    }
    let temporary = path.with_extension("ron.new");
    fs::write(&temporary, text).map_err(error)?;
    fs::rename(&temporary, path).map_err(error)
}

#[cfg(test)]
mod tests {
    use super::super::tree::{CENTRAL, Place, Side, Zone};
    use super::*;

    /// A directory of the test's own, emptied, and removed as the test ends, failed or not.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gg-dock-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_file_is_in_each_os_directory_for_per_user_state() {
        let env = |vars: &'static [(&str, &str)]| {
            move |name: &str| {
                let (_, value) = vars.iter().find(|(var, _)| *var == name)?;
                Some(OsString::from(value))
            }
        };
        let windows = env(&[("LOCALAPPDATA", r"C:\Users\Ada\AppData\Local")]);
        assert_eq!(
            path("Sandbox", "windows", windows),
            Some(Path::new(r"C:\Users\Ada\AppData\Local").join("GGEngine/Sandbox/layout.ron"))
        );
        let home = Path::new("/home/ada");
        let macos = env(&[("HOME", "/home/ada")]);
        assert_eq!(
            path("Sandbox", "macos", macos),
            Some(home.join("Library/Application Support/GGEngine/Sandbox/layout.ron"))
        );
        let linux = env(&[("HOME", "/home/ada"), ("XDG_STATE_HOME", "/state")]);
        assert_eq!(
            path("Sandbox", "linux", linux),
            Some(Path::new("/state").join("ggengine/Sandbox/layout.ron"))
        );
        // Without XDG_STATE_HOME, or with a relative one, it's under the home directory.
        for linux in [
            env(&[("HOME", "/home/ada")]),
            env(&[("HOME", "/home/ada"), ("XDG_STATE_HOME", "state")]),
        ] {
            assert_eq!(
                path("Sandbox", "linux", linux),
                Some(home.join(".local/state/ggengine/Sandbox/layout.ron"))
            );
        }
        assert_eq!(path("Sandbox", "windows", env(&[])), None);
    }

    #[test]
    fn a_saved_layout_loads_as_it_was() {
        let mut layout = Layout::new();
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
        let dir = Scratch::new("round-trip");
        let file = dir.0.join("layout.ron");
        save(&file, &layout).expect("the layout is saved");
        let mut loaded = load(&file).expect("the layout loads");
        layout.renumber();
        loaded.renumber();
        assert_eq!(loaded, layout);
        assert!(!file.with_extension("ron.new").exists());
    }

    #[test]
    fn a_missing_file_is_the_empty_layout_and_a_broken_one_an_error() {
        let dir = Scratch::new("broken");
        let file = dir.0.join("layout.ron");
        assert_eq!(load(&file).ok(), Some(Layout::new()));
        fs::create_dir_all(&dir.0).expect("a directory");
        // Text that isn't RON, and RON that isn't a layout the dock can use.
        for text in [
            "not a layout",
            "(main: Split(axis: Horizontal, ratio: 0.5, children: (Central, Central)), floating: [])",
        ] {
            fs::write(&file, text).expect("the file is written");
            let error = load(&file).expect_err("the file holds no usable layout");
            assert_eq!(error.code(), "engine.dock_layout");
            assert!(error.to_string().contains("layout.ron"), "{error}");
        }
    }
}
