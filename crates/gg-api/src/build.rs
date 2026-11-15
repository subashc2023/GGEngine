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
//! It also reads the game's project manifest, beside its `Cargo.toml`. On Windows, built by MSVC's
//! linker, it gives the game's executables version info, which names the game as
//! [`Project::name`] does, and the manifest's icon, if it names one, in a resource file it links in.
//! With an icon, it tells the game's code the icon's path, `GG_ICON`, which its `main` gives its
//! app's window, `.icon(include_bytes!(env!("GG_ICON")))`.
//!
//! The digest reads each file by its name and its bytes, in the order of their names. A file of the
//! game's workspace is named by its path from the workspace's root, `/` between its parts; one of a
//! workspace outside it, which a path dependency reaches, by `/`, the workspace's number, counting
//! from 1 in the order they're reached, and its path from that workspace's root, so that a checkout
//! of the engine beside a game digests alike wherever the two are. It reads:
//!
//! - the game's package's, and those of each package it reaches through path dependencies, normal
//!   or build ones, on any target, as `cargo metadata --no-deps` lists them in each one's own
//!   workspace: every file under its `src/` but those whose names begin with `.`, as macOS's
//!   `.DS_Store`, its `build.rs` and its `Cargo.toml`;
//! - the game's workspace's `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` and
//!   `.cargo/config.toml`, those there are, and each outside workspace's `Cargo.toml`, which its
//!   packages inherit from, the game's build reading no other file of it;
//!
//! and then `rustc -vV`'s answer but its `host` line, the game's features, and its flags to rustc,
//! `CARGO_ENCODED_RUSTFLAGS`. It leaves out where the workspaces are, the target and the OS, which
//! the simulation doesn't depend on, as CI's reference hashes check on each platform; files outside
//! `src/`, as shaders, which only render, and assets, which the game's package holds and the hello
//! names apart; the profile's levels of optimization and debug info, at which floats give the same
//! bits, debug and release told apart by the hello's engine; and the C and C++ compilers, which
//! differ by OS, CI's hashes checking Luau's results across them. A crate whose simulation code is
//! made from files outside its `src/` would have to be read here too. A dependency by git is read
//! through `Cargo.lock`, which names its commit; a `[patch]` that puts a path in a dependency's
//! place isn't read, as `cargo metadata --no-deps` doesn't list patches.
//!
//! For gg-engine's build script, [`check_cxx_contract`] refuses C++ flags that would let Luau's
//! scripts give other bits on one platform than on another.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{Failure, Project};

mod resource;

/// For a game's build script: tells cargo `GG_SOURCE`, the digest of what the game's code is
/// compiled from, as 16 hex digits, as the module says, and to run the script again as any file it
/// reads changes, the manifest and its icon among them; with an icon, `GG_ICON`, its absolute path;
/// and for a Windows target built by MSVC, to link the game's executables with the resource file
/// `gg.res` it writes in its `OUT_DIR`, holding their version info and any icon, as the module
/// says.
///
/// Fails with `build.metadata` if cargo's environment for build scripts, `cargo metadata` or
/// `rustc -vV` doesn't give what it should; with `build.read` if a file can't be read, naming it; as
/// gg-api's [`Project::read`] does for the manifest, with `project.read` or `project.bad`; with
/// `build.icon` for an icon that isn't a PNG of 256 by 256 pixels; with `build.version` for a
/// package's version that Windows' version info can't hold; and with `build.write` if the resource
/// file can't be written.
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
    let dir = PathBuf::from(var("CARGO_MANIFEST_DIR")?);
    let source = source(&Given {
        cargo: var("CARGO")?,
        rustc: var("RUSTC")?,
        dir: dir.clone(),
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

    let project = Project::read(&dir)?;
    println!(
        "cargo::rerun-if-changed={}",
        dir.join(Project::FILE).display()
    );
    let icon = match project.icon_path(&dir) {
        Some(path) => {
            println!("cargo::rerun-if-changed={}", path.display());
            let png = fs::read(&path).map_err(|error| unreadable(&path, &error))?;
            check_icon(&path, &png)?;
            println!("cargo::rustc-env=GG_ICON={}", path.display());
            Some(png)
        }
        None => None,
    };
    let windows = var("CARGO_CFG_TARGET_OS")? == "windows";
    if windows && var("CARGO_CFG_TARGET_ENV")? == "msvc" {
        let version = var("CARGO_PKG_VERSION")?;
        let version = version.to_string_lossy();
        let resources = resource::resources(&project.name(), &version, icon.as_deref())?;
        let path = PathBuf::from(var("OUT_DIR")?).join("gg.res");
        fs::write(&path, resources).map_err(|error| {
            let message = format!("can't write {}: {error}", path.display());
            Failure::new("build.write", message)
        })?;
        // link.exe takes a resource file as it takes an object.
        println!("cargo::rustc-link-arg-bins={}", path.display());
    }
    Ok(())
}

/// Checks that `png`, the bytes of the icon at `path`, are a PNG of 256 by 256 pixels, by its
/// signature and its first chunk, its header, which gives its size.
///
/// Fails with `build.icon` if they aren't.
fn check_icon(path: &Path, png: &[u8]) -> Result<(), Failure> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let header = (png.starts_with(SIGNATURE) && png.get(12..16) == Some(b"IHDR"))
        .then(|| png.get(16..24))
        .flatten();
    let size = header.map(|size| {
        let side =
            |at: usize| u32::from_be_bytes([size[at], size[at + 1], size[at + 2], size[at + 3]]);
        [side(0), side(4)]
    });
    match size {
        Some([256, 256]) => Ok(()),
        Some([width, height]) => {
            let message = format!(
                "the icon {} is {width} by {height} pixels, not 256 by 256, as Windows and macOS \
                 take a game's icon",
                path.display()
            );
            Err(Failure::new("build.icon", message))
        }
        None => {
            let message = format!("the icon {} isn't a PNG", path.display());
            Err(Failure::new("build.icon", message))
        }
    }
}

/// For gg-engine's build script: checks that Luau's C++, which mlua builds through luau0-src and the
/// cc crate, compiles for `target` on `host` with `-ffp-contract=off`, fusing no multiplication into
/// an addition, so that scripts give the same bits on every platform, as the engine's
/// `.cargo/config.toml` says. `var` gives an environment variable's value, `None` where it's unset.
///
/// The flag must come from `LUAU_CXXFLAGS`, the one variable whose change builds Luau again, as
/// luau0-src tells cargo; cc, whose cargo metadata luau0-src turns off, tells it of none of its own
/// variables. cc 1.9 passes the compiler luau0-src's flags, `LUAU_CXXFLAGS`' among them, and after
/// them its own variables', from the first to the last of `CXXFLAGS`, `HOST_CXXFLAGS`
/// (`TARGET_CXXFLAGS` for a target other than the host), `CXXFLAGS_` and the target with its `-`
/// and `.` as `_`, and `CXXFLAGS_` and the target, a flag's last instance winning; so none of those
/// may give another `-ffp-contract=`. Each is split at whitespace, as cc splits them unless
/// `CC_SHELL_ESCAPED_FLAGS` has it read them as a shell would. A target built by MSVC passes: it
/// takes no such flag, and fuses none by default.
///
/// Fails with `build.flags`, saying what to change.
pub fn check_cxx_contract(
    target: &str,
    host: &str,
    var: impl Fn(&str) -> Option<String>,
) -> Result<(), Failure> {
    if target.ends_with("-msvc") {
        return Ok(());
    }
    const OFF: &str = "-ffp-contract=off";
    let kind = if target == host { "HOST" } else { "TARGET" };
    let names = [
        "LUAU_CXXFLAGS".to_owned(),
        "CXXFLAGS".to_owned(),
        format!("{kind}_CXXFLAGS"),
        format!("CXXFLAGS_{}", target.replace(['-', '.'], "_")),
        format!("CXXFLAGS_{target}"),
    ];
    // Each variable's last `-ffp-contract=`, in the order the compiler gets them.
    let flags: Vec<(&str, Option<String>)> = (names.iter())
        .map(|name| {
            let value = var(name).unwrap_or_default();
            let mut flags = value.split_ascii_whitespace();
            let last = flags.rfind(|flag| flag.starts_with("-ffp-contract="));
            (name.as_str(), last.map(str::to_owned))
        })
        .collect();
    let effect = "so that the compiler could fuse a script's multiplications into additions, and \
                  the script give other bits than on other platforms";
    if flags[0].1.as_deref() != Some(OFF) {
        let message = format!(
            "Luau's C++ would build for {target} without `{OFF}` in `LUAU_CXXFLAGS`, {effect}: set \
             `LUAU_CXXFLAGS = \"{OFF}\"` under `[env]` in the workspace's `.cargo/config.toml`, as \
             the engine's does, or end the `LUAU_CXXFLAGS` you set with the flag. It goes there \
             rather than in a `CXXFLAGS`, as Luau builds again when `LUAU_CXXFLAGS` changes, and \
             not when those do."
        );
        return Err(Failure::new("build.flags", message));
    }
    let later = (flags[1..].iter().rev()).find_map(|(name, flag)| Some((name, flag.as_deref()?)));
    if let Some((name, flag)) = later
        && flag != OFF
    {
        let message = format!(
            "`{name}` gives `{flag}`, which cc passes Luau's compiler for {target} after \
             `LUAU_CXXFLAGS`, {effect}: take it out, and then run `cargo clean -p mlua-sys`, as \
             Luau doesn't build again when `{name}` changes."
        );
        return Err(Failure::new("build.flags", message));
    }
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
    // The game's workspace, then each outside it that a path dependency reaches, in the order
    // reached, which numbers their files' names.
    let mut workspaces = vec![metadata(&given.cargo, &given.dir)?];
    let mut files = BTreeMap::new();
    let mut watched = Vec::new();
    let mut seen = BTreeSet::new();
    let game = (workspaces[0].packages.iter()).position(|package| package.name == given.package);
    let Some(game) = game else {
        let message = format!("`cargo metadata` lists no package `{}`", given.package);
        return Err(Failure::new("build.metadata", message));
    };
    // Each package to read, by its workspace's place in `workspaces` and its own in that.
    let mut packages = vec![(0, game)];
    while let Some((at, index)) = packages.pop() {
        let workspace = &workspaces[at];
        let (root, package) = (&workspace.workspace_root, &workspace.packages[index]);
        if !seen.insert(package.manifest_path.clone()) {
            continue;
        }
        let prefix = prefix(at);
        let dir = package.manifest_path.parent().unwrap_or(root);
        read_dir(root, &prefix, &dir.join("src"), &mut files)?;
        watched.push(dir.join("src"));
        for file in [dir.join("build.rs"), package.manifest_path.clone()] {
            if file.is_file() {
                files.insert(format!("{prefix}{}", relative(root, &file)?), file.clone());
                watched.push(file);
            }
        }
        let name = package.name.clone();
        let paths: Vec<PathBuf> = (package.dependencies.iter())
            .filter(|dependency| dependency.kind.as_deref() != Some("dev"))
            .filter_map(|dependency| dependency.path.clone())
            .collect();
        for path in paths {
            let found = match find(&workspaces, &path) {
                Some(found) => found,
                None => {
                    workspaces.push(metadata(&given.cargo, &path)?);
                    find(&workspaces, &path).ok_or_else(|| {
                        let message = format!(
                            "`{name}` depends on the package at {}, which `cargo metadata` there \
                             doesn't list",
                            path.display()
                        );
                        Failure::new("build.metadata", message)
                    })?
                }
            };
            packages.push(found);
        }
    }
    for (at, workspace) in workspaces.iter().enumerate() {
        // An outside workspace's packages inherit from its `Cargo.toml`, the one file of it beside
        // theirs that the game's build reads: the lock, toolchain and cargo's config it builds with
        // are the game's workspace's.
        let names: &[&str] = if at == 0 {
            &[
                "Cargo.toml",
                "Cargo.lock",
                "rust-toolchain.toml",
                ".cargo/config.toml",
            ]
        } else {
            &["Cargo.toml"]
        };
        for name in names {
            let file = workspace.workspace_root.join(name);
            if file.is_file() {
                files.insert(format!("{}{name}", prefix(at)), file.clone());
                watched.push(file);
            }
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

/// The place among `workspaces` of the workspace, and of its package, whose directory is `dir`.
fn find(workspaces: &[Metadata], dir: &Path) -> Option<(usize, usize)> {
    workspaces.iter().enumerate().find_map(|(at, workspace)| {
        let mut packages = workspace.packages.iter();
        let index = packages.position(|package| package.manifest_path.parent() == Some(dir))?;
        Some((at, index))
    })
}

/// What the names of the files of the digest's `at`th workspace begin with: nothing for the
/// game's, the first; for one outside it, `/`, which no path from a root begins with, and its
/// number.
fn prefix(at: usize) -> String {
    if at == 0 {
        String::new()
    } else {
        format!("/{at}/")
    }
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

/// Adds each file under `dir` to `files`, by `prefix` and its path from `root`, as [`relative`] has
/// it, but those whose names begin with `.`.
fn read_dir(
    root: &Path,
    prefix: &str,
    dir: &Path,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), Failure> {
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
            read_dir(root, prefix, &path, files)?;
        } else {
            files.insert(format!("{prefix}{}", relative(root, &path)?), path);
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
    /// script, and uses `tested` in its tests alone; and beside it, in `engine`, a workspace of its
    /// own, whose `front` the game depends on by path, and which depends on `core` there.
    fn workspace(root: &Path) {
        let manifest = |name: &str, more: &str| {
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{more}")
        };
        let engine = root.parent().unwrap().join("engine");
        let _ = fs::remove_dir_all(&engine);
        for (path, text) in [
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"front\", \"core\"]\nresolver = \"3\"\n".to_owned(),
            ),
            ("Cargo.lock", "version = 4\n".to_owned()),
            ("README.md", "An engine.\n".to_owned()),
            (
                "front/Cargo.toml",
                manifest("front", "\n[dependencies]\ncore = { path = \"../core\" }\n"),
            ),
            ("front/src/lib.rs", "\n".to_owned()),
            ("core/Cargo.toml", manifest("core", "")),
            ("core/src/lib.rs", "\n".to_owned()),
        ] {
            let path = engine.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
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
                    "\n[dependencies]\nlib = { path = \"../lib\" }\nfront = { path = \"../../engine/front\" }\n\n[dev-dependencies]\ntested = { path = \"../tested\" }\n",
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
        assert_eq!(ours.digest, digest(&there), "where the workspaces are");
        let engine = base.join("engine");
        let mut watched: Vec<String> = (ours.watched.iter())
            .map(|path| match path.strip_prefix(&engine) {
                Ok(_) => format!("../engine/{}", relative(&engine, path).unwrap()),
                Err(_) => relative(&here, path).unwrap(),
            })
            .collect();
        watched.sort();
        let expected = [
            "../engine/Cargo.toml",
            "../engine/core/Cargo.toml",
            "../engine/core/src",
            "../engine/front/Cargo.toml",
            "../engine/front/src",
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
            ("../engine/README.md", "Another.\n"),
            (
                "../engine/Cargo.lock",
                "version = 4\n\n[[package]]\nname = \"core\"\nversion = \"0.1.0\"\n",
            ),
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
            ("../engine/core/src/lib.rs", "pub const MORE: u8 = 1;\n"),
            (
                "../engine/Cargo.toml",
                "[workspace]\nmembers = [\"front\", \"core\"]\nresolver = \"3\"\n\n[workspace.package]\nversion = \"0.1.0\"\n",
            ),
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

    /// A PNG's signature and header, of `width` by `height` pixels, as far as an icon is checked.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(width.to_be_bytes());
        png.extend(height.to_be_bytes());
        png.extend([8, 6, 0, 0, 0]);
        png
    }

    #[test]
    fn an_icon_is_a_png_of_256_by_256_pixels() {
        let path = Path::new("icon.png");
        assert_eq!(check_icon(path, &png(256, 256)), Ok(()));
        let mut not_png = png(256, 256);
        not_png[1] = b'J';
        for refused in [
            png(255, 256),
            png(256, 512),
            not_png,
            b"GIF89a".to_vec(),
            Vec::new(),
        ] {
            let code = check_icon(path, &refused).err().map(|failure| failure.code);
            assert_eq!(code.as_deref(), Some("build.icon"), "{refused:?}");
        }
    }

    #[test]
    fn luau_builds_with_no_fused_multiply_add_from_luau_s_flags_and_none_after() {
        let linux = "x86_64-unknown-linux-gnu";
        let (off, fast) = ("-O2 -ffp-contract=off", "-ffp-contract=fast -g");
        let luau = ("LUAU_CXXFLAGS", off);
        let check = |target: &str, host: &str, vars: &[(&str, &str)]| {
            let var = |name: &str| {
                let set = vars.iter().find(|(set, _)| *set == name);
                set.map(|(_, value)| (*value).to_owned())
            };
            check_cxx_contract(target, host, var)
        };
        let failure = check(linux, linux, &[]).expect_err("no flags");
        assert_eq!(failure.code, "build.flags");
        let fix = "`LUAU_CXXFLAGS = \"-ffp-contract=off\"`";
        assert!(failure.message.contains(fix), "{}", failure.message);
        assert_eq!(check("aarch64-pc-windows-msvc", linux, &[]), Ok(()));

        let on_linux = |vars: &[(&str, &str)]| check(linux, linux, vars).is_ok();
        let (underscored, dashed) = (
            "CXXFLAGS_x86_64_unknown_linux_gnu",
            "CXXFLAGS_x86_64-unknown-linux-gnu",
        );
        assert!(on_linux(&[luau]));
        assert!(on_linux(&[luau, ("CXXFLAGS", "-O3 -march=native")]));
        // Luau doesn't build again as cc's variables change.
        assert!(!on_linux(&[(underscored, off)]));
        let undone = "-ffp-contract=off -ffp-contract=on";
        assert!(!on_linux(&[("LUAU_CXXFLAGS", undone)]));
        // cc's variables, after Luau's, the last instance winning.
        for name in ["CXXFLAGS", "HOST_CXXFLAGS", underscored, dashed] {
            assert!(!on_linux(&[luau, (name, fast)]), "{name}");
        }
        assert!(on_linux(&[luau, ("CXXFLAGS", fast), (dashed, off)]));
        assert!(!on_linux(&[luau, ("CXXFLAGS", off), (dashed, fast)]));
        // `TARGET_CXXFLAGS` for another target than the host's, `HOST_CXXFLAGS` for its own.
        assert!(on_linux(&[luau, ("TARGET_CXXFLAGS", fast)]));
        let mac = "aarch64-apple-darwin";
        assert!(check(mac, linux, &[luau, ("TARGET_CXXFLAGS", fast)]).is_err());
        assert!(check(mac, linux, &[luau, ("HOST_CXXFLAGS", fast)]).is_ok());
        let dotted = "thumbv8m.main-none-eabi";
        let its = "CXXFLAGS_thumbv8m_main_none_eabi";
        assert!(check(dotted, dotted, &[luau, (its, fast)]).is_err());
    }
}
