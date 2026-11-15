//! `gg new`: a new game, as ARCHITECTURE.md's Game code describes: its package, its build script,
//! its project, an empty start scene, and a main module whose layer loads it. In the engine's
//! workspace it's a member, at `games/<NAME>`; elsewhere it's a workspace of its own, at `<NAME>`,
//! which builds against the engine `--engine` names, with copies of the engine's settings.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

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

[build-dependencies]
# Digests what the game's code is compiled from, which its sessions name its build by.
gg-api.workspace = true

[features]
default = ["dev"]
# A development build, which serves the tool API, applies its scene file's saves and reloads its
# assets as they change. A build without it runs from the game's package, as shipping builds do.
dev = ["gg-engine/dev"]

[lints]
workspace = true
"#;

/// The workspace of a game outside the engine's, which [`MANIFEST`]'s package inherits from, before
/// it in the game's manifest: `{engine}` standing for the lines [`Source::dependencies`] gives, and
/// its lints the engine's workspace's.
const WORKSPACE: &str = r#"# The game's own workspace: the crates its members use, the engine's among them, declared once here,
# each member naming them with `workspace = true`, as in the engine's workspace.
[workspace]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"
publish = false

[workspace.dependencies]
# The engine the game builds against, as `gg new --engine` named it.
{engine}

[workspace.lints.rust]
# A crate or one of its modules opts in to unsafe code with a stated reason, as the engine's
# AGENTS.md says.
unsafe_code = "deny"

[workspace.lints.clippy]
undocumented_unsafe_blocks = "warn"

"#;

/// The game's build script.
const BUILD: &str = r#"//! Tells the game's code the digest of what it's compiled from, `GG_SOURCE`, which its sessions name
//! its build by, and gives its Windows executable version info, as gg-api's `build` says.

fn main() -> Result<(), gg_api::Failure> {
    gg_api::build::emit()
}
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
    let app = App::new("{name}")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
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

/// The AGENTS.md of a game outside the engine's workspace, for agents in its repository: `{name}`
/// standing for the game's name, then `{engine}` and `{install}` for what [`Source`] says.
const AGENTS: &str = r#"# {name}

A game made with GGEngine, a workspace of its own, which builds against
{engine}.

The engine's rules hold here as in its own games. Read, in the engine: `AGENTS.md`, how to work with
it; `ARCHITECTURE.md`, its design, and what keeps a simulation the same on every platform;
gg-engine's root, `crates/gg-engine/src/lib.rs`, what games use; and `docs/LUAU.md`, what scripts
can use.

`gg`, the engine's command-line tool, runs here as in the engine's workspace: `gg run` runs the game
supervised, reloading it as it's edited; `gg check` runs it headless; `gg ship` lays it out as it
ships, in `target/ship`, once cargo-about 0.9.2 is installed and the game's manifest names its
license, which its notices list; `gg help` says the rest. Install `gg` from the engine the game
builds against:

```sh
{install}
```

A change is done when these pass:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy --no-default-features --all-targets -- -D warnings
cargo test
```

`.cargo/config.toml`, but for the engine's `cargo gg` alias, `clippy.toml`, `rust-toolchain.toml`,
`about.toml` and `.gitattributes` are the engine's, and `Cargo.lock` started as the engine's: the
settings its games build with, Luau's C++ flags among them; the float methods whose bits differ by
platform; its Rust release; the licenses a shipped game's notices accept; and LF line endings, as
the digest a session names a build by reads files' bytes. Keep them as the engine's are, copying
them again as the game moves to another revision of the engine: `gg check` warns (`gg.settings`)
where `clippy.toml` lacks a method the engine's bans, or `rust-toolchain.toml` pins another release.
"#;

/// The engine's own files that a game outside its workspace gets copies of, as `gg` was built with
/// them.
const ENGINE_CONFIG: &str = include_str!("../../../.cargo/config.toml");
const ENGINE_CLIPPY: &str = include_str!("../../../clippy.toml");
const ENGINE_TOOLCHAIN: &str = include_str!("../../../rust-toolchain.toml");
const ENGINE_ABOUT: &str = include_str!("../../../about.toml");
const ENGINE_ATTRIBUTES: &str = include_str!("../../../.gitattributes");
const ENGINE_IGNORE: &str = include_str!("../../../.gitignore");
const ENGINE_CLAUDE: &str = include_str!("../../../CLAUDE.md");
const ENGINE_LOCK: &str = include_str!("../../../Cargo.lock");

/// The project of new game `name`: its directory its asset root, its scenes in `scenes`, and its
/// start scene `scenes/main.ron`.
pub(crate) fn project(name: &str) -> Project {
    Project {
        game: name.to_owned(),
        assets: ".".to_owned(),
        scenes: vec!["scenes".to_owned()],
        start: "scenes/main.ron".to_owned(),
        relay: None,
        icon: None,
    }
}

/// The engine a game outside the engine's workspace builds against, as `--engine` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Engine {
    /// An engine checkout's directory, from the working directory or absolute.
    Path(PathBuf),
    /// An engine's git repository, by its URL, at a revision.
    Git { url: String, rev: String },
}

impl Engine {
    /// The engine `given`, `--engine`'s value, names: a git repository, `<URL>#<REVISION>`, for a
    /// URL, which has `://`, as `https://`, `ssh://` and `file://` do; else a checkout's directory.
    ///
    /// Fails, saying why, for nothing given or a URL without its revision.
    pub(crate) fn parse(given: &str) -> Result<Self, String> {
        if !given.contains("://") {
            if given.is_empty() {
                return Err(
                    "`--engine` takes an engine checkout's directory, or `<GIT URL>#<REVISION>`"
                        .to_owned(),
                );
            }
            return Ok(Self::Path(PathBuf::from(given)));
        }
        match given.rsplit_once('#') {
            Some((url, rev)) if !url.is_empty() && !rev.is_empty() => Ok(Self::Git {
                url: url.to_owned(),
                rev: rev.to_owned(),
            }),
            _ => Err(format!(
                "a git engine is `<URL>#<REVISION>`, pinned to a revision, not `{given}`"
            )),
        }
    }

    /// What a new game in a folder below `here` writes of the engine.
    ///
    /// Fails as [`checkout`] does.
    fn source(&self, here: &Path) -> Result<Source, Failure> {
        match self {
            Self::Path(given) => checkout(given, here),
            Self::Git { url, rev } => {
                let git = format!(
                    "{{ git = {}, rev = {} }}",
                    toml_string(url),
                    toml_string(rev)
                );
                Ok(Source {
                    dependencies: format!("gg-engine = {git}\ngg-api = {git}"),
                    engine: format!("the engine's repository, `{url}`, at revision `{rev}`"),
                    install: format!("cargo install --locked --git \"{url}\" --rev \"{rev}\" gg"),
                })
            }
        }
    }
}

/// What a new game in a folder below `here` writes of the engine checkout at `given`, from `here`
/// or absolute: its path from the game's directory, or, absolute, as it is.
///
/// Fails with `gg.new` if `given` holds no gg-engine or gg-api, or if its path isn't valid Unicode.
fn checkout(given: &Path, here: &Path) -> Result<Source, Failure> {
    let failed = |message: String| Failure::new("gg.new", message);
    for member in ["gg-engine", "gg-api"] {
        let manifest = here.join(given).join("crates").join(member);
        if !manifest.join("Cargo.toml").is_file() {
            return Err(failed(format!(
                "{} holds no engine checkout, without `crates/{member}/Cargo.toml`; a git engine \
                 is a URL, as `ssh://git@host/owner/repo.git#<REVISION>`",
                here.join(given).display()
            )));
        }
    }
    let relative = given.components().all(|part| {
        matches!(
            part,
            Component::Normal(_) | Component::CurDir | Component::ParentDir
        )
    });
    let at: PathBuf = if relative {
        let parts = given.components().filter(|&part| part != Component::CurDir);
        Path::new("..").components().chain(parts).collect()
    } else {
        let absolute = std::path::absolute(given)
            .map_err(|error| failed(format!("can't tell where {} is: {error}", given.display())))?;
        absolute.components().collect()
    };
    let at =
        (at.to_str()).ok_or_else(|| failed(format!("{} isn't valid Unicode", at.display())))?;
    // Cargo takes `/` on Windows too, and it reads more easily there.
    let at = if cfg!(windows) {
        at.replace('\\', "/")
    } else {
        at.to_owned()
    };
    let path = |member: &str| toml_string(&format!("{at}/crates/{member}"));
    Ok(Source {
        dependencies: format!(
            "gg-engine = {{ path = {} }}\ngg-api = {{ path = {} }}",
            path("gg-engine"),
            path("gg-api")
        ),
        engine: format!("the engine's checkout at `{at}`"),
        install: format!("cargo install --locked --path \"{at}/tools/gg\""),
    })
}

/// What a new game outside the engine's workspace writes of the engine it builds against.
struct Source {
    /// Its manifest's `[workspace.dependencies]` lines for the engine's crates, gg-engine and
    /// gg-api.
    dependencies: String,
    /// Where its AGENTS.md says the engine is.
    engine: String,
    /// The command its AGENTS.md gives to install gg from the engine.
    install: String,
}

/// Writes new game `name`, whose [`project`] checks, and prints a line of JSON naming its directory
/// and files. In the engine's workspace, the one cargo finds from the working directory if it has
/// gg-engine as a member, it's a member, in `games`, taking no `engine`; elsewhere, a workspace of
/// its own in the working directory, building against `engine`, which it then needs.
///
/// Fails with `gg.new` for `engine` given in the engine's workspace or left out elsewhere, if a
/// package of the engine's workspace or its lock has the name, writing nothing, as [`write_files`]
/// does, if a game in the workspace doesn't join it, keeping its files, or as [`checkout`] does;
/// and with `gg.cargo` if a workspace is there that cargo can't say what it is.
pub(crate) fn new(name: &str, engine: Option<&Engine>) -> Result<(), Failure> {
    let failed = |message: String| Failure::new("gg.new", message);
    if locked(name) {
        return Err(failed(format!(
            "the engine's lock has a package `{name}`, which cargo couldn't tell from the game's, \
             as `--package {name}`, with which gg builds a game, would name both"
        )));
    }
    let here = std::env::current_dir()
        .map_err(|error| failed(format!("can't tell which directory gg runs in: {error}")))?;
    // Where no directory up to the root has a manifest, cargo finds no workspace, and says so as an
    // error.
    let manifest = here.ancestors().any(|dir| dir.join("Cargo.toml").is_file());
    let workspace = manifest.then(|| build::metadata(false, None)).transpose()?;
    let in_engine = workspace.filter(|metadata| has(metadata, "gg-engine"));
    let (dir, files) = match (in_engine, engine) {
        (Some(metadata), None) => {
            if has(&metadata, name) {
                return Err(failed(format!(
                    "the workspace has a package `{name}` already"
                )));
            }
            let root = metadata["workspace_root"].as_str().ok_or_else(|| {
                Failure::new("gg.cargo", "`cargo metadata` named no workspace root")
            })?;
            let dir = Path::new(root).join("games").join(name);
            let files = write_files(&dir, &files(name))?;
            if !has(&build::metadata(false, None)?, name) {
                return Err(failed(format!(
                    "{} doesn't join the workspace, whose members leave out `games/*`; its files \
                     are kept",
                    dir.display()
                )));
            }
            (dir, files)
        }
        (Some(_), Some(_)) => {
            return Err(failed(
                "in the engine's workspace, a new game is a member, in `games`, building against \
                 the workspace's engine: `--engine` is for a game of its own outside it"
                    .to_owned(),
            ));
        }
        (None, Some(engine)) => {
            let source = engine.source(&here)?;
            let dir = here.join(name);
            let files = write_files(&dir, &files_outside(name, &source))?;
            (dir, files)
        }
        (None, None) => {
            return Err(failed(
                "outside the engine's workspace, a new game is a workspace of its own, which takes \
                 the engine it builds against: `--engine <PATH>`, an engine checkout's directory, \
                 or `--engine <GIT URL>#<REVISION>`"
                    .to_owned(),
            ));
        }
    };
    crate::print(&json!({"new": name, "dir": dir, "files": files}).to_string());
    Ok(())
}

/// Whether the workspace that `metadata`, what `cargo metadata` said, describes has package
/// `name`.
fn has(metadata: &Value, name: &str) -> bool {
    let packages = metadata["packages"].as_array();
    packages.is_some_and(|packages| packages.iter().any(|package| package["name"] == name))
}

/// Whether the engine's `Cargo.lock` has a package named `name`: one of its own, or a crate it
/// builds with, which a game, in the workspace or outside, builds with too.
fn locked(name: &str) -> bool {
    let names = ENGINE_LOCK
        .lines()
        .filter_map(|line| line.strip_prefix("name = \""));
    names
        .filter_map(|rest| rest.strip_suffix('"'))
        .any(|locked| locked == name)
}

/// New game `name`'s files in the engine's workspace, by their paths from its directory, `/`
/// between their parts, with their text: its package's manifest, its build script, its project's,
/// its start scene and its main module.
fn files(name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Cargo.toml", MANIFEST.replace("{name}", name)),
        ("build.rs", BUILD.to_owned()),
        (Project::FILE, project(name).text()),
        ("scenes/main.ron", SCENE.to_owned()),
        ("src/main.rs", MAIN.replace("{name}", name)),
    ]
}

/// New game `name`'s files outside the engine's workspace, as [`files`] gives them: its manifest
/// with a workspace of its own before its package, whose dependencies on the engine `source` gives;
/// copies of the engine's settings, its config without the `cargo gg` alias, which runs gg from the
/// engine's source and so works in the engine's workspace alone; the engine's `Cargo.lock`, which
/// the game's starts from, so that the crates under the engine resolve as its tests ran them; the
/// engine's `.gitignore`; and an AGENTS.md and the engine's CLAUDE.md, which imports it.
fn files_outside(name: &str, source: &Source) -> Vec<(&'static str, String)> {
    let workspace = WORKSPACE.replace("{engine}", &source.dependencies);
    let mut files: Vec<(&str, String)> = (files(name).into_iter())
        .map(|(file, text)| match file {
            "Cargo.toml" => (file, workspace.clone() + &text),
            _ => (file, text),
        })
        .collect();
    let agents = (AGENTS.replace("{name}", name))
        .replace("{engine}", &source.engine)
        .replace("{install}", &source.install);
    files.extend([
        (".cargo/config.toml", config().to_owned()),
        ("clippy.toml", ENGINE_CLIPPY.to_owned()),
        ("rust-toolchain.toml", ENGINE_TOOLCHAIN.to_owned()),
        ("about.toml", ENGINE_ABOUT.to_owned()),
        (".gitattributes", ENGINE_ATTRIBUTES.to_owned()),
        ("Cargo.lock", ENGINE_LOCK.to_owned()),
        (".gitignore", ENGINE_IGNORE.to_owned()),
        ("AGENTS.md", agents),
        ("CLAUDE.md", ENGINE_CLAUDE.to_owned()),
    ]);
    files
}

/// The engine's `.cargo/config.toml` but for its first paragraph, the `cargo gg` alias.
fn config() -> &'static str {
    let (_alias, rest) = (ENGINE_CONFIG.split_once("\n\n"))
        .expect("the engine's config has more after its alias, as gg's tests check");
    rest
}

/// `text` as a TOML basic string, quoted, with what TOML needs escaped.
fn toml_string(text: &str) -> String {
    let mut quoted = String::from('"');
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(quoted, "\\u{:04X}", u32::from(c));
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Writes `files`, each a path from directory `dir`, `/` between its parts, and its text, in `dir`,
/// writing over nothing, and returns their paths.
///
/// Fails with `gg.new` if `dir` is there, writing nothing, or if a file can't be written, keeping
/// those written.
fn write_files(dir: &Path, files: &[(&str, String)]) -> Result<Vec<PathBuf>, Failure> {
    let failed = |message: String| Failure::new("gg.new", message);
    if dir.exists() {
        return Err(failed(format!("{} is there already", dir.display())));
    }
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

    /// The engine's workspace's root, two folders above gg's.
    fn root() -> &'static Path {
        let gg = Path::new(env!("CARGO_MANIFEST_DIR"));
        gg.ancestors()
            .nth(2)
            .expect("gg is at tools/gg in the workspace")
    }

    /// The lines that set a key in table `header` of manifest `text`, its comments and blank lines
    /// left out.
    fn table<'a>(text: &'a str, header: &str) -> Vec<&'a str> {
        let lines = text.lines().skip_while(|&line| line != header).skip(1);
        let lines = lines.take_while(|line| !line.starts_with('['));
        let keys = lines.filter(|line| !line.is_empty() && !line.starts_with('#'));
        keys.collect()
    }

    #[test]
    fn a_new_game_is_its_manifest_its_project_its_main_and_an_empty_scene() {
        let games = std::env::temp_dir().join(format!("gg-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&games);
        let dir = games.join("probe");
        let files = write_files(&dir, &files("probe")).expect("written");
        let read = |file: &str| fs::read_to_string(dir.join(file)).expect("written");
        let (manifest, build, scene, main) = (
            read("Cargo.toml"),
            read("build.rs"),
            read("scenes/main.ron"),
            read("src/main.rs"),
        );
        let project = Project::read(&dir);
        // Into a directory that's there, nothing is written.
        let there = games.join("there");
        fs::create_dir_all(&there).expect("a directory");
        let refused = write_files(&there, &super::files("there"))
            .err()
            .map(|failure| failure.code);
        let empty = fs::read_dir(&there).expect("there").next().is_none();
        let _ = fs::remove_dir_all(&games);
        assert_eq!(files.len(), 5);
        assert_eq!(project, Ok(super::project("probe")));
        assert!(manifest.contains("name = \"gg-probe\""), "{manifest}");
        // Its sessions name its build by its source, as its build script digests it.
        assert!(
            manifest.contains("[build-dependencies]\n# Digests") && build.contains("emit()"),
            "{manifest}{build}"
        );
        assert_eq!(scene, SCENE);
        assert!(main.contains("App::new(\"probe\")"), "{main}");
        assert!(main.contains(".source(env!(\"GG_SOURCE\"))"), "{main}");
        // A start scene that doesn't load ends the run with its error, as runtime failures do,
        // rather than panicking.
        assert!(
            main.contains("cx.fail(error)") && !main.contains("panic!"),
            "{main}"
        );
        assert_eq!(refused.as_deref(), Some("gg.new"));
        assert!(empty);
    }

    /// Outside the engine's workspace, a new game is a workspace of its own, its package as in the
    /// workspace, building against the engine by its path from the game, with copies of the
    /// engine's files as they are, but for the config's `cargo gg` alias.
    #[test]
    fn a_game_outside_is_a_workspace_of_its_own_with_the_engines_settings() {
        let here = std::env::temp_dir().join(format!("gg-new-outside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&here);
        for member in ["gg-engine", "gg-api"] {
            let crate_dir = here.join("engine").join("crates").join(member);
            fs::create_dir_all(&crate_dir).expect("a directory");
            fs::write(crate_dir.join("Cargo.toml"), "").expect("written");
        }
        let source = Engine::Path(PathBuf::from("./engine/")).source(&here);
        let dir = here.join("probe");
        let written = source.map(|source| write_files(&dir, &files_outside("probe", &source)));
        let read = |file: &str| fs::read_to_string(dir.join(file)).expect("written");
        let files = [
            "Cargo.toml",
            ".cargo/config.toml",
            "clippy.toml",
            "rust-toolchain.toml",
            "about.toml",
            ".gitattributes",
            "Cargo.lock",
            ".gitignore",
            "AGENTS.md",
            "CLAUDE.md",
        ];
        let [
            manifest,
            config,
            clippy,
            toolchain,
            about,
            attributes,
            lock,
            ignore,
            agents,
            claude,
        ] = files.map(read);
        let project = Project::read(&dir);
        let _ = fs::remove_dir_all(&here);
        let engine = |file: &str| fs::read_to_string(root().join(file)).expect("the engine's");
        assert_eq!(written.expect("an engine").expect("written").len(), 14);
        assert_eq!(project, Ok(super::project("probe")));
        // The package as in the engine's workspace, after a workspace of its own, which names the
        // engine by its path from the game's directory.
        assert!(manifest.starts_with(WORKSPACE.split("{engine}").next().expect("a head")));
        assert!(manifest.ends_with(&MANIFEST.replace("{name}", "probe")));
        assert!(
            manifest.contains(concat!(
                "gg-engine = { path = \"../engine/crates/gg-engine\" }\n",
                "gg-api = { path = \"../engine/crates/gg-api\" }\n"
            )),
            "{manifest}"
        );
        // The engine's lints, as each member takes them with `workspace = true`.
        for header in ["[workspace.lints.rust]", "[workspace.lints.clippy]"] {
            let engines = engine("Cargo.toml");
            assert_eq!(
                table(&manifest, header),
                table(&engines, header),
                "{header}"
            );
            assert!(!table(&manifest, header).is_empty(), "{header}");
        }
        // The engine's config but its alias, which runs gg in the engine's workspace alone.
        let engines = engine(".cargo/config.toml");
        let alias = engines.strip_suffix(&config).expect("the config's end");
        assert!(alias.starts_with("[alias]\n"), "{alias}");
        assert!(alias.contains("--package gg"), "{alias}");
        assert!(!config.contains("[alias]"), "{config}");
        assert!(config.contains("[env]\nLUAU_CXXFLAGS"), "{config}");
        assert!(config.contains("[profile.shipping]"), "{config}");
        for (copy, file) in [
            (&clippy, "clippy.toml"),
            (&toolchain, "rust-toolchain.toml"),
            (&about, "about.toml"),
            (&attributes, ".gitattributes"),
            (&lock, "Cargo.lock"),
            (&ignore, ".gitignore"),
            (&claude, "CLAUDE.md"),
        ] {
            assert_eq!(copy, &engine(file), "{file}");
        }
        assert!(claude.contains("@AGENTS.md"), "{claude}");
        assert!(agents.starts_with("# probe\n"), "{agents}");
        assert!(
            agents.contains("\nthe engine's checkout at `../engine`.\n"),
            "{agents}"
        );
        assert!(
            agents.contains("```sh\ncargo install --locked --path \"../engine/tools/gg\"\n```"),
            "{agents}"
        );
    }

    /// The engine `--engine` names: a checkout, by its path from the game a folder below, as
    /// written, or absolute; or a git repository at a revision. A checkout without the engine's
    /// crates is refused.
    #[test]
    fn engines_are_checkouts_by_path_or_git_repositories_at_a_revision() {
        let parsed = |given: &str| Engine::parse(given);
        let git = Engine::Git {
            url: "https://example.com/engine.git".to_owned(),
            rev: "4f2a91c".to_owned(),
        };
        assert_eq!(
            parsed("https://example.com/engine.git#4f2a91c"),
            Ok(git.clone())
        );
        assert_eq!(
            parsed("../engine"),
            Ok(Engine::Path(PathBuf::from("../engine")))
        );
        for refused in [
            "",
            "https://example.com/engine.git",
            "https://example.com/engine.git#",
        ] {
            assert!(parsed(refused).is_err(), "{refused}");
        }
        let source = git.source(Path::new("nowhere")).expect("a source");
        assert_eq!(
            source.dependencies,
            concat!(
                "gg-engine = { git = \"https://example.com/engine.git\", rev = \"4f2a91c\" }\n",
                "gg-api = { git = \"https://example.com/engine.git\", rev = \"4f2a91c\" }"
            )
        );
        assert_eq!(
            source.install,
            "cargo install --locked --git \"https://example.com/engine.git\" --rev \"4f2a91c\" gg"
        );

        let here = std::env::temp_dir().join(format!("gg-new-engines-{}", std::process::id()));
        let _ = fs::remove_dir_all(&here);
        let crates = here.join("work").join("engine").join("crates");
        for member in ["gg-engine", "gg-api"] {
            fs::create_dir_all(crates.join(member)).expect("a directory");
            fs::write(crates.join(member).join("Cargo.toml"), "").expect("written");
        }
        let from = |given: &Path| Engine::Path(given.to_owned()).source(&here.join("work"));
        let up = from(Path::new("../work/engine")).map(|source| source.dependencies);
        let absolute = from(&here.join("work").join("engine")).map(|source| source.dependencies);
        fs::remove_file(crates.join("gg-api").join("Cargo.toml")).expect("removed");
        let without = from(Path::new("engine")).err().map(|failure| failure.code);
        let _ = fs::remove_dir_all(&here);
        let up = up.expect("a source");
        assert!(up.starts_with("gg-engine = { path = \"../../work/engine/crates/gg-engine\" }"));
        let absolute = absolute.expect("a source");
        let at = here.join("work").join("engine").join("crates");
        let at = at.to_str().expect("a UTF-8 path").replace('\\', "/");
        assert!(
            absolute.contains(&toml_string(&format!("{at}/gg-api"))),
            "{absolute}"
        );
        assert_eq!(without.as_deref(), Some("gg.new"));
        assert_eq!(toml_string("a\"b\\c\u{7f}"), "\"a\\\"b\\\\c\\u007F\"");
    }

    /// The names of the engine's crates and those it builds with are taken: `--package` would name
    /// the game's and theirs alike.
    #[test]
    fn the_names_in_the_engines_lock_are_taken() {
        for taken in ["gg", "gg-api", "gg-engine", "glam", "serde"] {
            assert!(locked(taken), "{taken}");
        }
        for free in ["probe", "gla"] {
            assert!(!locked(free), "{free}");
        }
    }
}
