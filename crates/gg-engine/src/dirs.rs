//! The OS's directories where an app keeps its files for the user, never beside the executable:
//! one for what it keeps from run to run, such as the dock's layout, and one for what it can make
//! again, such as its pipeline cache; and writing a file whole, there or, as a scene's, anywhere.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Writes `data` to the file at `path` whole, as [`write_whole`] does, making the directories it's
/// in.
pub(crate) fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_whole(path, data)
}

/// Writes `data` to the file at `path` whole or not at all, so that a crash or the power going
/// while it writes leaves the last one whole: to a file of its own beside the one `path` names,
/// through any link, which it flushes to the disk and renames over that one, with its permissions.
/// It refuses a file that's read only, on every OS, as Windows' rename would; replaces one with
/// other hard links rather than writing through it, so that they keep what it held; makes no
/// directories; and removes what it wrote if it fails.
pub(crate) fn write_whole(path: &Path, data: &[u8]) -> io::Result<()> {
    // The file a link names, so that the link stays one, and its permissions, which the file
    // renamed over it takes rather than the umask's.
    let (target, permissions) = match fs::canonicalize(path) {
        Ok(target) => {
            let permissions = fs::metadata(&target)?.permissions();
            if permissions.readonly() {
                let message = "it's read only";
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, message));
            }
            (target, Some(permissions))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (path.to_owned(), None),
        Err(e) => return Err(e),
    };
    let (temporary, file) = create_beside(&target)?;
    let written = flush(file, data, permissions).and_then(|()| fs::rename(&temporary, &target));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// A file new to the disk beside `target`, named after it with this process's ID, a count and
/// `.new` after, so that none already there, such as one a user named so, is emptied or replaced.
fn create_beside(target: &Path) -> io::Result<(PathBuf, File)> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    loop {
        let mut name = target.as_os_str().to_owned();
        let count = COUNT.fetch_add(1, Ordering::Relaxed);
        name.push(format!(".{}-{count}.new", std::process::id()));
        let name = PathBuf::from(name);
        match File::options().write(true).create_new(true).open(&name) {
            Ok(file) => return Ok((name, file)),
            // One a run that crashed left, whose process had this one's ID.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
}

/// Writes `data` to `file`, gives it `permissions`, if any, and flushes it to the disk, closing it.
fn flush(mut file: File, data: &[u8], permissions: Option<fs::Permissions>) -> io::Result<()> {
    file.write_all(data)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.sync_all()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A directory of a test's own in the OS's temporary directory, emptied but not made, and
    /// removed as the test ends, failed or not.
    pub(crate) struct Scratch(pub(crate) PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gg-{name}-{}", std::process::id()));
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
