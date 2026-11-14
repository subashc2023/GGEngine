//! For a game's build script: the digest of what the game's code is compiled from, which a session's
//! hello names the game's build by, with its package's, so that runs of one source join whatever OS
//! and machine built them, and runs of another are refused, as ARCHITECTURE.md's Networking says. A
//! game's `build.rs` is
//!
//! ```no_run
//! fn main() -> Result<(), gg_api::Failure> {
//!     gg_api::build::emit()
//! }
//! ```
//!
//! and its `main` names the digest to its app, `.source(env!("GG_SOURCE"))`.
//!
//! The digest reads each file by its path from the workspace's root, `/` between its parts, and its
//! bytes, in the order of their paths:
//!
//! - the game's package's, and those of each package of the workspace it reaches through path
//!   dependencies, normal or build ones, on any target, as `cargo metadata` lists them: every file
//!   under its `src/` but those whose names begin with `.`, as macOS's `.DS_Store`, its `build.rs`
//!   and its `Cargo.toml`;
//! - the workspace's `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` and `.cargo/config.toml`,
//!   those there are;
//!
//! and then `rustc -vV`'s answer but its `host` line, the game's features, and its flags to rustc,
//! `CARGO_ENCODED_RUSTFLAGS`. It leaves out where the workspace is, the target and the OS, which the
//! simulation doesn't depend on, as CI's reference hashes check on each platform; files outside
//! `src/`, as shaders, which only render, and assets, which the game's package holds and the hello
//! names apart; the profile's levels of optimization and debug info, at which floats give the same
//! bits, debug and release told apart by the hello's engine; and the C and C++ compilers, which
//! differ by OS, CI's hashes checking Luau's results across them. A crate whose simulation code is
//! made from files outside its `src/` would have to be read here too.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::Failure;

/// For a game's build script: tells cargo `GG_SOURCE`, the digest of what the game's code is
/// compiled from, as 16 hex digits, as the module says, and to run the script again as any file it
/// reads changes.
///
/// Fails with `build.metadata` if cargo's environment for build scripts, `cargo metadata` or
/// `rustc -vV` doesn't give what it should, and with `build.read` if a file can't be read, naming
/// it.
pub fn emit() -> Result<(), Failure> {
    let var = |name: &str| {
        env::var_os(name).ok_or_else(|| {
            let message = format!("cargo set no `{name}`, as it does for a build script");
            Failure::new("build.metadata", message)
        })
    };
    let mut features: Vec<String> = env::vars_os()
        .filter_map(|(key, _)| Some(key.to_str()?.strip_prefix("CARGO_FEATURE_")?.to_owned()))
        .collect();
    features.sort();
    let source = source(&Given {
        cargo: var("CARGO")?,
        rustc: var("RUSTC")?,
        dir: var("CARGO_MANIFEST_DIR")?.into(),
        package: var("CARGO_PKG_NAME")?.to_string_lossy().into_owned(),
        features,
        rustflags: var("CARGO_ENCODED_RUSTFLAGS")?
            .to_string_lossy()
            .into_owned(),
    })?;
    for path in &source.watched {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    println!("cargo::rustc-env=GG_SOURCE={:016x}", source.digest);
    Ok(())
}

/// What cargo tells a package's build script, which the digest reads.
struct Given {
    /// The cargo and the rustc of the build.
    cargo: OsString,
    rustc: OsString,
    /// The package's directory and its name.
    dir: PathBuf,
    package: String,
    /// Its features, as their variables name them, `CARGO_FEATURE_DEV`'s `DEV`, in order.
    features: Vec<String>,
    /// Its flags to rustc, as `CARGO_ENCODED_RUSTFLAGS` gives them.
    rustflags: String,
}

/// What a package's code is compiled from: its digest, and the files and directories the digest
/// read, which cargo watches.
struct Source {
    digest: u64,
    watched: Vec<PathBuf>,
}

/// The workspace, as `cargo metadata --no-deps` gives it: its root and its packages.
#[derive(Deserialize)]
struct Metadata {
    workspace_root: PathBuf,
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    /// `dev`, `build`, or `None` for a normal one.
    kind: Option<String>,
    /// The package's directory, for a path dependency.
    path: Option<PathBuf>,
}

/// The digest of what `given`'s package is compiled from, as the module says.
fn source(given: &Given) -> Result<Source, Failure> {
    let workspace = metadata(&given.cargo, &given.dir)?;
    let root = &workspace.workspace_root;
    let mut files = BTreeMap::new();
    let mut watched = Vec::new();
    let mut seen = BTreeSet::new();
    let game = (workspace.packages.iter()).find(|package| package.name == given.package);
    let mut packages: Vec<&Package> = game.into_iter().collect();
    if packages.is_empty() {
        let message = format!("`cargo metadata` lists no package `{}`", given.package);
        return Err(Failure::new("build.metadata", message));
    }
    while let Some(package) = packages.pop() {
        if !seen.insert(&package.manifest_path) {
            continue;
        }
        let dir = package.manifest_path.parent().unwrap_or(root);
        read_dir(root, &dir.join("src"), &mut files)?;
        watched.push(dir.join("src"));
        for file in [dir.join("build.rs"), package.manifest_path.clone()] {
            if file.is_file() {
                files.insert(relative(root, &file)?, file.clone());
                watched.push(file);
            }
        }
        let paths = (package.dependencies.iter())
            .filter(|dependency| dependency.kind.as_deref() != Some("dev"))
            .filter_map(|dependency| dependency.path.as_deref());
        for path in paths {
            let found = (workspace.packages.iter())
                .find(|package| package.manifest_path.parent() == Some(path));
            let Some(found) = found else {
                let message = format!(
                    "`{}` depends on the package at {}, which isn't a member of the workspace, \
                     whose members alone `cargo metadata --no-deps` lists",
                    package.name,
                    path.display()
                );
                return Err(Failure::new("build.metadata", message));
            };
            packages.push(found);
        }
    }
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        ".cargo/config.toml",
    ] {
        let file = root.join(name);
        if file.is_file() {
            files.insert(name.to_owned(), file.clone());
            watched.push(file);
        }
    }

    let mut digest = Fnv::new();
    for (name, path) in &files {
        let bytes = fs::read(path).map_err(|error| unreadable(path, &error))?;
        digest.part(name.as_bytes());
        digest.part(&bytes);
    }
    digest.part(toolchain(&given.rustc)?.as_bytes());
    digest.part(&(given.features.len() as u64).to_le_bytes());
    for feature in &given.features {
        digest.part(feature.as_bytes());
    }
    digest.part(given.rustflags.as_bytes());
    Ok(Source {
        digest: digest.0,
        watched,
    })
}

/// The workspace of the package in `dir`, as `cargo metadata --no-deps` lists it, which resolves
/// no dependency, and so needs no network and takes no lock a build holds.
fn metadata(cargo: &OsStr, dir: &Path) -> Result<Metadata, Failure> {
    let failed = |why: String| {
        let message = format!("can't list the workspace's packages, `cargo metadata`: {why}");
        Failure::new("build.metadata", message)
    };
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .output()
        .map_err(|error| failed(error.to_string()))?;
    if !output.status.success() {
        return Err(failed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| failed(error.to_string()))
}

/// Adds each file under `dir` to `files`, by its path from `root`, as [`relative`] has it, but
/// those whose names begin with `.`.
fn read_dir(root: &Path, dir: &Path, files: &mut BTreeMap<String, PathBuf>) -> Result<(), Failure> {
    let entries = fs::read_dir(dir).map_err(|error| unreadable(dir, &error))?;
    for entry in entries {
        let path = entry.map_err(|error| unreadable(dir, &error))?.path();
        if path
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
        {
            continue;
        }
        if path.is_dir() {
            read_dir(root, &path, files)?;
        } else {
            files.insert(relative(root, &path)?, path);
        }
    }
    Ok(())
}

/// `path`'s path from `root`, `/` between its parts, the same on every OS.
fn relative(root: &Path, path: &Path) -> Result<String, Failure> {
    let relative = path.strip_prefix(root).map_err(|_| {
        let message = format!(
            "{} isn't under the workspace's root, {}",
            path.display(),
            root.display()
        );
        Failure::new("build.metadata", message)
    })?;
    let parts: Vec<_> = relative.iter().map(OsStr::to_string_lossy).collect();
    Ok(parts.join("/"))
}

/// `rustc -vV`'s answer, which names its release, its commit and its LLVM's, but its `host` line,
/// which names the OS.
fn toolchain(rustc: &OsStr) -> Result<String, Failure> {
    let failed = |why: String| {
        let message = format!("can't ask {} its version: {why}", rustc.display());
        Failure::new("build.metadata", message)
    };
    let output =
        (Command::new(rustc).arg("-vV").output()).map_err(|error| failed(error.to_string()))?;
    if !output.status.success() {
        return Err(failed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let text = String::from_utf8(output.stdout).map_err(|error| failed(error.to_string()))?;
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("host:"))
        .collect();
    Ok(lines.join("\n"))
}

fn unreadable(path: &Path, error: &std::io::Error) -> Failure {
    Failure::new(
        "build.read",
        format!("can't read {}: {error}", path.display()),
    )
}

/// FNV-1a's 64 bits: small, and the same on every platform, as the digest must be.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    /// `bytes`, after their length, so that where one part ends and the next begins is digested
    /// too.
    fn part(&mut self, bytes: &[u8]) {
        let len = (bytes.len() as u64).to_le_bytes();
        for &byte in len.iter().chain(bytes) {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace at `root`: a game, `game`, which depends on `lib` by path and is built by a
    /// script, and uses `tested` in its tests alone.
    fn workspace(root: &Path) {
        let manifest = |name: &str, more: &str| {
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{more}")
        };
        let files = [
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"game\", \"lib\", \"tested\"]\nresolver = \"3\"\n"
                    .to_owned(),
            ),
            ("Cargo.lock", "version = 4\n".to_owned()),
            ("README.md", "A workspace.\n".to_owned()),
            (
                "game/Cargo.toml",
                manifest(
                    "game",
                    "\n[dependencies]\nlib = { path = \"../lib\" }\n\n[dev-dependencies]\ntested = { path = \"../tested\" }\n",
                ),
            ),
            ("game/build.rs", "fn main() {}\n".to_owned()),
            ("game/src/main.rs", "fn main() {}\n".to_owned()),
            ("game/assets/arena.ron", "()\n".to_owned()),
            ("lib/Cargo.toml", manifest("lib", "")),
            ("lib/src/lib.rs", "pub const SPEED: f32 = 2.0;\n".to_owned()),
            ("lib/src/deep/more.rs", "\n".to_owned()),
            ("tested/Cargo.toml", manifest("tested", "")),
            ("tested/src/lib.rs", "\n".to_owned()),
            ("target/debug/game", "built\n".to_owned()),
        ];
        let _ = fs::remove_dir_all(root);
        for (path, text) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    fn given(root: &Path, features: &[&str]) -> Given {
        Given {
            cargo: env!("CARGO").into(),
            rustc: "rustc".into(),
            dir: root.join("game"),
            package: "game".to_owned(),
            features: features.iter().map(|&feature| feature.to_owned()).collect(),
            rustflags: String::new(),
        }
    }

    #[test]
    fn the_digest_reads_the_game_s_code_and_the_workspace_s_wherever_they_are() {
        let base = env::temp_dir().join(format!("gg-api-build-{}", std::process::id()));
        let (here, there) = (base.join("here"), base.join("elsewhere").join("deeper"));
        workspace(&here);
        workspace(&there);
        let digest = |root: &Path| source(&given(root, &["DEV"])).expect("digested").digest;
        let ours = source(&given(&here, &["DEV"])).expect("digested");
        assert_eq!(ours.digest, digest(&there), "where the workspace is");
        let mut watched: Vec<String> = (ours.watched.iter())
            .map(|path| relative(&here, path).unwrap())
            .collect();
        watched.sort();
        let expected = [
            "Cargo.lock",
            "Cargo.toml",
            "game/Cargo.toml",
            "game/build.rs",
            "game/src",
            "lib/Cargo.toml",
            "lib/src",
        ];
        assert_eq!(watched, expected);

        // What the game isn't compiled from.
        for (path, text) in [
            ("README.md", "Another.\n"),
            ("game/assets/arena.ron", "(changed)\n"),
            ("tested/src/lib.rs", "pub fn changed() {}\n"),
            ("target/debug/game", "rebuilt\n"),
            ("lib/src/.DS_Store", "Finder's\n"),
        ] {
            fs::write(there.join(path), text).unwrap();
            assert_eq!(ours.digest, digest(&there), "{path}");
        }
        // What it is.
        let mut seen = vec![ours.digest];
        for (path, text) in [
            ("lib/src/deep/more.rs", "pub fn more() {}\n"),
            (
                "Cargo.lock",
                "version = 4\n\n[[package]]\nname = \"game\"\nversion = \"0.1.0\"\n",
            ),
            ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.99.1\"\n"),
        ] {
            fs::write(there.join(path), text).unwrap();
            let changed = digest(&there);
            assert!(!seen.contains(&changed), "{path}");
            seen.push(changed);
        }
        let featured = source(&given(&there, &["DEV", "TRACY"])).expect("digested");
        assert!(!seen.contains(&featured.digest), "features");
        fs::remove_dir_all(&base).unwrap();
    }
}
