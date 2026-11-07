//! `gg new`: a new game in the workspace, at `games/<NAME>`, as ARCHITECTURE.md's Game code
//! describes: its package, its project, an empty start scene, and a main module whose layer loads
//! it.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use gg_api::{Failure, Project};
use serde_json::{Value, json};

use crate::build;

/// The game's package's manifest, `{name}` standing for the game's name, as in [`MAIN`].
const MANIFEST: &str = r#"[package]
name = "{name}"
version.workspace = true
edition.workspace = true
publish.workspace = true

# GPU drivers apply per-game profiles by executable name: engine executables get a `gg-` prefix to
# stay clear of such matches.
[[bin]]
name = "gg-{name}"
path = "src/main.rs"

[dependencies]
gg-engine.workspace = true

[features]
default = ["dev"]
# A development build, which serves the tool API, applies its scene file's saves and reloads its
# assets as they change. A build without it runs from the game's package, as shipping builds do.
dev = ["gg-engine/dev"]

[lints]
workspace = true
"#;

/// The game's main module.
const MAIN: &str = r#"//! {name}, a game made with GGEngine.
//!
//! Its world starts as its project's start scene, `scenes/main.ron`, which a development build
//! applies as it's saved.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::process::ExitCode;

use gg_engine::{App, Context, Layer};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance and Inspector panels: what `main` runs, and what its
/// recordings replay.
fn app() -> App {
    let app = App::new("{name}").project(env!("CARGO_MANIFEST_DIR"));
    let app = app.layer(Game).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// The game's layer.
struct Game;

impl Layer for Game {
    fn attach(&mut self, cx: &mut Context) {
        // Watched as the game runs, in development builds, which apply its saves. One that doesn't
        // load ends the run with its error.
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }
}
"#;

/// A scene without entities, in its canonical text.
const SCENE: &str = "(\n    entities: {},\n)\n";

/// The project of new game `name`: its directory its asset root, its scenes in `scenes`, and its
/// start scene `scenes/main.ron`.
pub(crate) fn project(name: &str) -> Project {
    Project {
        game: name.to_owned(),
        assets: ".".to_owned(),
        scenes: vec!["scenes".to_owned()],
        start: "scenes/main.ron".to_owned(),
    }
}

/// Writes new game `name`, whose [`project`] checks, in `games` in the workspace cargo finds from
/// the working directory, and prints a line of JSON naming its directory and files.
///
/// Fails with `gg.new` if a package of the workspace has the name, writing nothing, as
/// [`write_files`] does, or if the game doesn't join the workspace, keeping its files; and with
/// `gg.cargo` if cargo can't say what the workspace is.
pub(crate) fn new(name: &str) -> Result<(), Failure> {
    let failed = |message: String| Failure::new("gg.new", message);
    let metadata = build::metadata(false, None)?;
    if has(&metadata, name) {
        return Err(failed(format!(
            "the workspace has a package `{name}` already"
        )));
    }
    let root = metadata["workspace_root"]
        .as_str()
        .ok_or_else(|| Failure::new("gg.cargo", "`cargo metadata` named no workspace root"))?;
    let dir = Path::new(root).join("games").join(name);
    let files = write_files(&dir, name)?;
    if !has(&build::metadata(false, None)?, name) {
        return Err(failed(format!(
            "{} doesn't join the workspace, whose members leave out `games/*`; its files are kept",
            dir.display()
        )));
    }
    crate::print(&json!({"new": name, "dir": dir, "files": files}).to_string());
    Ok(())
}

/// Whether the workspace that `metadata`, what `cargo metadata` said, describes has package
/// `name`.
fn has(metadata: &Value, name: &str) -> bool {
    let packages = metadata["packages"].as_array();
    packages.is_some_and(|packages| packages.iter().any(|package| package["name"] == name))
}

/// Writes new game `name`'s files in directory `dir`, writing over nothing, and returns their
/// paths: its package's manifest, its project's, its start scene and its main module.
///
/// Fails with `gg.new` if `dir` is there, writing nothing, or if a file can't be written, keeping
/// those written.
fn write_files(dir: &Path, name: &str) -> Result<Vec<PathBuf>, Failure> {
    let failed = |message: String| Failure::new("gg.new", message);
    if dir.exists() {
        return Err(failed(format!("{} is there already", dir.display())));
    }
    let files = [
        ("Cargo.toml", MANIFEST.replace("{name}", name)),
        (Project::FILE, project(name).text()),
        ("scenes/main.ron", SCENE.to_owned()),
        ("src/main.rs", MAIN.replace("{name}", name)),
    ];
    let mut written = Vec::new();
    for (file, text) in files {
        let path = file
            .split('/')
            .fold(dir.to_owned(), |path, part| path.join(part));
        let wrote = fs::create_dir_all(path.parent().unwrap_or(dir))
            .and_then(|()| File::create_new(&path))
            .and_then(|mut created| created.write_all(text.as_bytes()));
        wrote.map_err(|error| failed(format!("can't write {}: {error}", path.display())))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_game_is_its_manifest_its_project_its_main_and_an_empty_scene() {
        let games = std::env::temp_dir().join(format!("gg-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&games);
        let dir = games.join("probe");
        let files = write_files(&dir, "probe").expect("written");
        let read = |file: &str| fs::read_to_string(dir.join(file)).expect("written");
        let (manifest, scene, main) = (
            read("Cargo.toml"),
            read("scenes/main.ron"),
            read("src/main.rs"),
        );
        let project = Project::read(&dir);
        // Into a directory that's there, nothing is written.
        let there = games.join("there");
        fs::create_dir_all(&there).expect("a directory");
        let refused = write_files(&there, "there")
            .err()
            .map(|failure| failure.code);
        let empty = fs::read_dir(&there).expect("there").next().is_none();
        let _ = fs::remove_dir_all(&games);
        assert_eq!(files.len(), 4);
        assert_eq!(project, Ok(super::project("probe")));
        assert!(manifest.contains("name = \"gg-probe\""), "{manifest}");
        assert_eq!(scene, SCENE);
        assert!(main.contains("App::new(\"probe\")"), "{main}");
        // A start scene that doesn't load ends the run with its error, as runtime failures do,
        // rather than panicking.
        assert!(
            main.contains("cx.fail(error)") && !main.contains("panic!"),
            "{main}"
        );
        assert_eq!(refused.as_deref(), Some("gg.new"));
        assert!(empty);
    }
}
