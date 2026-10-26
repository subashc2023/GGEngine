//! The OS's directories where an app keeps its files for the user, never beside the executable:
//! one for what it keeps from run to run, such as the dock's layout, and one for what it can make
//! again, such as its pipeline cache.

use std::ffi::OsString;
use std::path::PathBuf;

/// Which of the per-user directories a file goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// What the app keeps for the user, such as the dock's layout.
    State,
    /// What the app can make again, which the user may delete, such as its pipeline cache.
    Cache,
}

/// Where app `app` keeps its files of `kind`: `%LOCALAPPDATA%\GGEngine\<app>` on Windows, both
/// kinds; on macOS `~/Library/Application Support/GGEngine/<app>`, and caches in
/// `~/Library/Caches/GGEngine/<app>`; and elsewhere `$XDG_STATE_HOME/ggengine/<app>`, by default
/// `~/.local/state/ggengine/<app>`, and caches in `$XDG_CACHE_HOME/ggengine/<app>`, by default
/// `~/.cache/ggengine/<app>`. `os` is [`std::env::consts::OS`], and `var` reads the environment.
/// `None` without the variables.
pub(crate) fn dir(
    app: &str,
    kind: Kind,
    os: &str,
    var: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let var = |name| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let dir = match (os, kind) {
        ("windows", _) => var("LOCALAPPDATA")?.join("GGEngine"),
        ("macos", Kind::State) => var("HOME")?.join("Library/Application Support/GGEngine"),
        ("macos", Kind::Cache) => var("HOME")?.join("Library/Caches/GGEngine"),
        (_, kind) => {
            let (variable, in_home) = match kind {
                Kind::State => ("XDG_STATE_HOME", ".local/state"),
                Kind::Cache => ("XDG_CACHE_HOME", ".cache"),
            };
            // The XDG spec says to ignore a relative path.
            var(variable)
                .filter(|dir| dir.as_os_str().as_encoded_bytes().starts_with(b"/"))
                .or_else(|| Some(var("HOME")?.join(in_home)))?
                .join("ggengine")
        }
    };
    Some(dir.join(app))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn each_kind_of_file_is_in_its_os_directory_for_the_user() {
        let env = |vars: &'static [(&str, &str)]| {
            move |name: &str| {
                let (_, value) = vars.iter().find(|(var, _)| *var == name)?;
                Some(OsString::from(value))
            }
        };
        let windows = env(&[("LOCALAPPDATA", r"C:\Users\Ada\AppData\Local")]);
        for kind in [Kind::State, Kind::Cache] {
            assert_eq!(
                dir("Sandbox", kind, "windows", windows),
                Some(Path::new(r"C:\Users\Ada\AppData\Local").join("GGEngine/Sandbox"))
            );
        }
        let home = Path::new("/home/ada");
        let macos = env(&[("HOME", "/home/ada")]);
        assert_eq!(
            dir("Sandbox", Kind::State, "macos", macos),
            Some(home.join("Library/Application Support/GGEngine/Sandbox"))
        );
        assert_eq!(
            dir("Sandbox", Kind::Cache, "macos", macos),
            Some(home.join("Library/Caches/GGEngine/Sandbox"))
        );
        let linux = env(&[
            ("HOME", "/home/ada"),
            ("XDG_STATE_HOME", "/state"),
            ("XDG_CACHE_HOME", "/cache"),
        ]);
        assert_eq!(
            dir("Sandbox", Kind::State, "linux", linux),
            Some(Path::new("/state/ggengine/Sandbox").to_owned())
        );
        assert_eq!(
            dir("Sandbox", Kind::Cache, "linux", linux),
            Some(Path::new("/cache/ggengine/Sandbox").to_owned())
        );
        // Without the XDG variables, or with relative ones, they're under the home directory.
        for linux in [
            env(&[("HOME", "/home/ada")]),
            env(&[
                ("HOME", "/home/ada"),
                ("XDG_STATE_HOME", "state"),
                ("XDG_CACHE_HOME", "cache"),
            ]),
        ] {
            assert_eq!(
                dir("Sandbox", Kind::State, "linux", linux),
                Some(home.join(".local/state/ggengine/Sandbox"))
            );
            assert_eq!(
                dir("Sandbox", Kind::Cache, "linux", linux),
                Some(home.join(".cache/ggengine/Sandbox"))
            );
        }
        assert_eq!(dir("Sandbox", Kind::State, "windows", env(&[])), None);
    }
}
