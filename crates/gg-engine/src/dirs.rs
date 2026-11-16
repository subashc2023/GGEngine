//! The OS's per-user directories for an app's files, never beside the executable: state it keeps
//! across runs (the dock's layout) and a cache it can remake. Also whole-file I/O: reading
//! regular files, writing (a scene), creating (an asset's ID file), and writing private files
//! (the tool API's session file).

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Which per-user directory a file goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    State,
    /// Remakable, so the user may delete it: the pipeline cache.
    Cache,
}

/// Where app `app` keeps files of `kind`:
///
/// - Windows, both kinds: `%LOCALAPPDATA%\GGEngine\<app>`.
/// - macOS: `~/Library/Application Support/GGEngine/<app>`; caches in
///   `~/Library/Caches/GGEngine/<app>`.
/// - Elsewhere: `$XDG_STATE_HOME/ggengine/<app>`, default `~/.local/state/ggengine/<app>`; caches
///   in `$XDG_CACHE_HOME/ggengine/<app>`, default `~/.cache/ggengine/<app>`.
///
/// `os` is [`std::env::consts::OS`]; `var` reads the environment. `None` without the variables.
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

/// Reads the first `most` bytes of the file at `path`. Regular files only, so a FIFO or device
/// such as /dev/zero can neither hang the read nor fill memory.
pub(crate) fn read_file(path: &Path, most: u64) -> io::Result<Vec<u8>> {
    // Before opening, since opening a FIFO waits for a writer.
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        let not_regular = "it isn't a regular file";
        return Err(io::Error::new(io::ErrorKind::InvalidInput, not_regular));
    }
    let mut bytes = Vec::new();
    // An error, not `fs::read`'s abort, if memory runs out.
    bytes.try_reserve_exact(metadata.len().min(most).try_into().unwrap_or(0))?;
    File::open(path)?.take(most).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Like [`write_whole`], but makes the directories.
pub(crate) fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_whole(path, data)
}

/// Writes `data` to `path` whole or not at all: a crash or power loss mid-write leaves the last
/// file whole. It writes a file beside the one `path` names (through any link), flushes it and
/// renames it over that one, keeping its permissions.
///
/// Refuses a read-only file on every OS, as Windows' rename would. Replaces a file with other hard
/// links rather than writing through it, so they keep what it held. Makes no directories; removes
/// what it wrote on failure.
pub(crate) fn write_whole(path: &Path, data: &[u8]) -> io::Result<()> {
    // The file a link names, so the link stays one; and its permissions, which the renamed file
    // takes instead of the umask's.
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
    let (temporary, file) = create_beside(&target, open_new)?;
    let written = flush(file, data, permissions).and_then(|()| fs::rename(&temporary, &target));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// Like [`write()`], but only the user the process runs as can read the file. It's written beside
/// `path`, the user's alone from creation, and renamed over whatever is at `path` (a link too),
/// taking none of its permissions. Unix: file mode 0600, new directories 0700. Windows: the
/// file's DACL allows its owner alone, as `windows_private` says. For the tool API's session
/// file, whose token unlocks the game.
#[cfg(feature = "dev")]
pub(crate) fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(fs::DirBuilder::new().recursive(true), 0o700)
            .create(dir)?;
        #[cfg(not(unix))]
        fs::create_dir_all(dir)?;
    }
    // The umask can only narrow the mode it's created with.
    #[cfg(unix)]
    let open_private = |path: &Path| {
        use std::os::unix::fs::OpenOptionsExt;
        File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    };
    #[cfg(target_os = "windows")]
    let open_private = crate::windows_private::create_new;
    let (temporary, file) = create_beside(path, open_private)?;
    let written = flush(file, data, None).and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// Writes `data` as a new file at `path`, whole or not at all; `AlreadyExists` if anything is
/// there. It's written beside `path`, flushed, hard-linked at `path` and removed either way, so
/// two processes never both write one and none reads one half-written. Makes no directories.
#[cfg(feature = "dev")]
pub(crate) fn create(path: &Path, data: &[u8]) -> io::Result<()> {
    let (temporary, file) = create_beside(path, open_new)?;
    let created = flush(file, data, None).and_then(|()| fs::hard_link(&temporary, path));
    let _ = fs::remove_file(&temporary);
    created
}

/// A new file beside `target`, made by `open`, which must fail with `AlreadyExists` if the name
/// is taken. The name is `target`'s plus process ID, count and `.new`, so no existing file is
/// emptied or replaced, even a user's.
fn create_beside(
    target: &Path,
    open: impl Fn(&Path) -> io::Result<File>,
) -> io::Result<(PathBuf, File)> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    loop {
        let mut name = target.as_os_str().to_owned();
        let count = COUNT.fetch_add(1, Ordering::Relaxed);
        name.push(format!(".{}-{count}.new", std::process::id()));
        let name = PathBuf::from(name);
        match open(&name) {
            Ok(file) => return Ok((name, file)),
            // Left by a crashed run whose process had this ID.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
}

fn open_new(path: &Path) -> io::Result<File> {
    File::options().write(true).create_new(true).open(path)
}

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

    /// A test's temporary directory: emptied, not made, removed as the test ends, failed or not.
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

    #[test]
    fn a_regular_file_is_read_as_far_as_asked_and_a_device_not_at_all() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let whole = fs::read(&manifest).expect("the manifest");
        assert_eq!(read_file(&manifest, 10).expect("its start"), whole[..10]);
        assert_eq!(read_file(&manifest, u64::MAX).expect("all of it"), whole);
        // A device would read as far as asked: zeros, or on Windows nothing.
        let device = if cfg!(windows) {
            "models/NUL"
        } else {
            "/dev/zero"
        };
        read_file(Path::new(device), 1 << 20).expect_err("a device");
    }

    /// A second create fails with `AlreadyExists` and keeps the first's bytes; neither leaves
    /// temporaries.
    #[cfg(feature = "dev")]
    #[test]
    fn a_file_is_created_whole_once_whoever_writes_first() {
        let scratch = Scratch::new("created-whole");
        fs::create_dir_all(&scratch.0).expect("a directory");
        let path = scratch.0.join("stone.png.meta");
        create(&path, b"first").expect("created");
        assert_eq!(fs::read(&path).expect("the file"), b"first");
        let second = create(&path, b"second").expect_err("there already");
        assert_eq!(second.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).expect("the file"), b"first");
        let names: Vec<String> = (fs::read_dir(&scratch.0).expect("listed"))
            .map(|entry| entry.expect("an entry").file_name().into_string())
            .collect::<Result<_, _>>()
            .expect("Unicode");
        assert_eq!(names, ["stone.png.meta"], "no file of its own left");
    }

    /// A private file is 0600 in 0700 directories, even replacing a file others could read.
    #[cfg(all(unix, feature = "dev"))]
    #[test]
    fn a_private_file_is_the_user_s_alone_whatever_was_there() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("private-mode");
        let dir = scratch.0.join("sessions");
        let path = dir.join("game-1.json");
        let mode = |path: &Path| fs::metadata(path).expect("there").permissions().mode() & 0o777;
        write_private(&path, b"first").expect("written");
        assert_eq!(
            [mode(&scratch.0), mode(&dir), mode(&path)],
            [0o700, 0o700, 0o600]
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("readable by all");
        write_private(&path, b"second").expect("written");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read(&path).expect("the file"), b"second");
    }
}
