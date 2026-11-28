//! `gg new`: a new game, as ARCHITECTURE.md's Game code describes. In the engine's workspace it's
//! a member at `games/<NAME>`; elsewhere a workspace of its own at `<NAME>`, building against the
//! `--engine` and with copies of the engine's settings.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use gg_api::{Failure, Project};
use serde_json::{Value, json};

use crate::build;

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

/// The workspace [`MANIFEST`] inherits from; `{engine}` is [`Source::dependencies`].
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
too_long_first_doc_paragraph = "warn"

"#;

const BUILD: &str = r#"//! Tells the game's code the digest of what it's compiled from, `GG_SOURCE`, which its sessions name
//! its build by, and gives its Windows executable version info, as gg-api's `build` says.

fn main() -> Result<(), gg_api::Failure> {
    gg_api::build::emit()
}
"#;

const MAIN: &str = r#"//! {name}, a game made with GGEngine.
//!
//! Its start scene, `scenes/main.ron`, holds a camera and the player, a square that the arrow keys,
//! WASD or a pad's left stick move. A development build applies the scene's saves as it runs.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::ops::ControlFlow;
use std::process::ExitCode;

use gg_engine::prelude::*;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's debug overlays: what `main` runs, what its recordings replay, and
/// what a restart in place builds again (`App::hot`).
fn app() -> App {
    let app = App::new("{name}")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"))
        .hot(app);
    app.layer(Game::new()).debug_overlays()
}

/// What the player steers: it moves `speed` units a second along `heading`.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    #[reflect(min = 0.0, default = 4.0)]
    speed: f32,
    /// As the last steer set it, of length at most 1.
    heading: Vec2,
}

/// What the keys and pads do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Turns every player toward `heading` until the next.
    Steer { heading: Vec2 },
}

/// What the keys and pads are bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Steer,
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
}

impl Game {
    fn new() -> Self {
        let mut actions = Actions::default();
        let steer = [Binding::WASD, Binding::ARROWS, Binding::stick(Stick::Left)];
        let bound = actions.bind_all(steer.map(|binding| (Act::Steer, binding)));
        bound.expect("the bindings are pairs of axes, of other sources");
        // What `gg actions` says each does, so an agent plays without reading this code. Name what
        // the value moves, its units, and the fields that set them or that it changes;
        // `describe_control` lists the mouse's look, clicks and wheel the same way. This runs as
        // the game starts, so an edit to a constant it reads restarts the game in place rather than
        // patching it: numbers to tune live best in components, which `gg set` changes as the game
        // runs.
        let steer = "Moves the player: [x, y], right and up; 1 is `Player.speed` units a second";
        actions.describe(Act::Steer, steer);
        Self {
            actions,
            commands: Vec::new(),
        }
    }
}

impl Layer for Game {
    /// Registers the player, which the start scene names.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        cx.world_mut().register::<Player>()
    }

    fn attach(&mut self, cx: &mut Context) {
        // Watched as the game runs, in development builds, which apply its saves. One that doesn't
        // load ends the run with its error.
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    /// Lets go of what was held as the world was taken, as when `gg run` restarts the game.
    fn restored(&mut self, _: &mut Context) {
        let heading = Vec2::ZERO;
        self.commands.push(Command::Steer { heading });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            match command {
                Command::Steer { heading } => {
                    for player in world.query_mut::<&mut Player>() {
                        player.heading = heading;
                    }
                }
            }
        }
        let dt = Tick::DT.as_secs_f32();
        for (player, at) in world.query_mut::<(&Player, &mut Transform)>() {
            let by = player.heading * player.speed * dt;
            at.translation += by.extend(0.0).as_dvec3();
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| {
            if let Change::Axis2(Act::Steer, heading) = change {
                commands.push(Command::Steer { heading });
            }
        });
        ControlFlow::Continue(())
    }
}
"#;

/// The start scene, in canonical text: a camera, and the player's square.
const SCENE: &str = r#"(
    entities: {
        "2f6c3a91d8e4b057": (
            Camera: (
                projection: Orthographic(
                    height: 10.0,
                    near: 0.1,
                    far: 100.0,
                ),
                primary: true,
            ),
            Name: ("Camera"),
            Transform: (
                translation: (0.0, 0.0, 10.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
        ),
        "7b1e94c2a05d63f8": (
            Name: ("Player"),
            Player: (
                speed: 4.0,
                heading: (0.0, 0.0),
            ),
            Sprite: (
                color: (0.2, 0.6, 1.0, 1.0),
                texture: None,
                rect: [
                    (0.0, 0.0),
                    (1.0, 1.0),
                ],
                sampler: LinearClamp,
            ),
            Transform: (
                translation: (0.0, 0.0, 0.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
        ),
    },
)
"#;

/// The AGENTS.md of a game outside the engine's workspace; `{engine}` and `{install}` come from
/// [`Source`].
const AGENTS: &str = r#"# {name}

A game made with GGEngine, a workspace of its own, which builds against
{engine}.

As `gg new` made it, its start scene, `scenes/main.ron`, holds a camera and a square with the
game's `Player` component. `src/main.rs` registers `Player`, binds the arrow keys, WASD and a pad's
left stick, and moves the square at each tick by the commands they queue. `gg actions` lists them
with what each does, as `Actions::describe` says it: keep that true as controls change.

The engine's rules hold here as in its own games. Read, in the engine: `AGENTS.md`, how to work with
it; `ARCHITECTURE.md`, its design, and what keeps a simulation the same on every platform;
gg-engine's root, `crates/gg-engine/src/lib.rs`, what games use; and `docs/LUAU.md`, what scripts
can use.

The engine's games, in its `games/`, show how. `topdown`, a 2D game in one file, binds actions,
spawns and removes entities, and draws a HUD from a UI file. It also draws random numbers from the
world, adds a debug overlay of its own, and tests seeded ticks. `fps` and `tps` are 3D, `cards` a
game of UI, and `showcase/scenes` holds a scene per feature.

`gg`, the engine's command-line tool, runs here as in the engine's workspace: `gg run` runs the game
supervised, reloading it as it's edited; `gg check` runs it headless; `gg ship` lays it out as it
ships, in `target/ship`, once cargo-about 0.9.2 is installed and the game's manifest names its
license, which its notices list; `gg help` says the rest. The engine's docs write `cargo gg`, its
alias in the engine's workspace: here, run `gg`. Install it from the engine the game builds
against:

```sh
{install}
```

Install it again as the game moves to another revision of the engine: `gg` warns (`gg.protocol`) as
it reaches a game whose engine speaks another revision of the tool API than its own.

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

/// Copied into games outside the engine's workspace, as `gg` was built with them.
const ENGINE_CONFIG: &str = include_str!("../../../.cargo/config.toml");
const ENGINE_CLIPPY: &str = include_str!("../../../clippy.toml");
const ENGINE_TOOLCHAIN: &str = include_str!("../../../rust-toolchain.toml");
const ENGINE_ABOUT: &str = include_str!("../../../about.toml");
const ENGINE_ATTRIBUTES: &str = include_str!("../../../.gitattributes");
const ENGINE_IGNORE: &str = include_str!("../../../.gitignore");
const ENGINE_CLAUDE: &str = include_str!("../../../CLAUDE.md");
const ENGINE_LOCK: &str = include_str!("../../../Cargo.lock");

/// The project of new game `name`.
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

/// The engine `--engine` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Engine {
    /// A checkout's directory, relative to the working directory or absolute.
    Path(PathBuf),
    /// A git repository at a revision.
    Git { url: String, rev: String },
}

impl Engine {
    /// Parses `--engine`: `<URL>#<REVISION>` if it has `://` (`https://`, `ssh://`, `file://`),
    /// else a checkout's directory.
    ///
    /// Fails for nothing given or a URL without a revision.
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

    /// What a game below `here` writes of the engine; fails as [`checkout`] does.
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

/// The checkout at `given` (from `here` or absolute), by its path from the game's directory, or
/// as is if absolute.
///
/// Fails with `gg.new` if `given` lacks gg-engine or gg-api, or its path isn't Unicode.
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
    // `/` on Windows too: cargo takes it, and it reads better.
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

struct Source {
    /// `[workspace.dependencies]` lines for gg-engine and gg-api.
    dependencies: String,
    /// Where AGENTS.md says the engine is.
    engine: String,
    /// AGENTS.md's command to install gg.
    install: String,
}

/// Writes new game `name`, printing a JSON line of its directory, its files and `next`, the command
/// that runs it. In the engine's workspace (cargo's from the working directory, if it has
/// gg-engine) it's a member in `games`, taking no `engine`; elsewhere a workspace in the working
/// directory, building against `engine`.
///
/// Fails with `gg.new` for:
/// - `engine` given in the engine's workspace, or missing outside it;
/// - `name` a package of the engine's workspace or lock, writing nothing;
/// - a game that doesn't join the workspace, keeping its files;
/// - the failures of [`write_files`] and [`checkout`].
///
/// Fails with `gg.cargo` if cargo can't describe a workspace there.
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
    // With no manifest up to the root, cargo finds no workspace and errors.
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
    // Outside the engine's workspace, from the directory gg ran in, as the game's is below it.
    let next = match engine {
        None => format!("cargo gg run {name}"),
        Some(_) => format!("gg run --project {name}"),
    };
    crate::print(&json!({"new": name, "dir": dir, "files": files, "next": next}).to_string());
    Ok(())
}

fn has(metadata: &Value, name: &str) -> bool {
    let packages = metadata["packages"].as_array();
    packages.is_some_and(|packages| packages.iter().any(|package| package["name"] == name))
}

/// Whether the engine's `Cargo.lock` has package `name`, its own or a dependency's.
fn locked(name: &str) -> bool {
    let names = ENGINE_LOCK
        .lines()
        .filter_map(|line| line.strip_prefix("name = \""));
    names
        .filter_map(|rest| rest.strip_suffix('"'))
        .any(|locked| locked == name)
}

/// New game `name`'s files in the engine's workspace, by `/`-separated path from its directory.
fn files(name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Cargo.toml", MANIFEST.replace("{name}", name)),
        ("build.rs", BUILD.to_owned()),
        (Project::FILE, project(name).text()),
        ("scenes/main.ron", SCENE.to_owned()),
        ("src/main.rs", MAIN.replace("{name}", name)),
    ]
}

/// New game `name`'s files outside the engine's workspace: [`files`] with a workspace before the
/// manifest, and copies of the engine's:
/// - settings, the config without the `cargo gg` alias, which works in the engine's workspace only;
/// - `Cargo.lock`, so the crates resolve as the engine's tests ran them;
/// - `.gitignore` and CLAUDE.md, which imports the game's AGENTS.md.
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

/// The engine's config without its first paragraph, the `cargo gg` alias.
fn config() -> &'static str {
    let (_alias, rest) = (ENGINE_CONFIG.split_once("\n\n"))
        .expect("the engine's config has more after its alias, as gg's tests check");
    rest
}

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

/// Writes `files` (by `/`-separated path from `dir`) in `dir`; returns their paths.
///
/// Fails with `gg.new` if `dir` is there, writing nothing, or a file can't be written, keeping
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

    fn root() -> &'static Path {
        let gg = Path::new(env!("CARGO_MANIFEST_DIR"));
        gg.ancestors()
            .nth(2)
            .expect("gg is at tools/gg in the workspace")
    }

    fn table<'a>(text: &'a str, header: &str) -> Vec<&'a str> {
        let lines = text.lines().skip_while(|&line| line != header).skip(1);
        let lines = lines.take_while(|line| !line.starts_with('['));
        let keys = lines.filter(|line| !line.is_empty() && !line.starts_with('#'));
        keys.collect()
    }

    #[test]
    fn a_new_game_is_its_manifest_its_project_its_main_and_its_start_scene() {
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
        // A directory that's there gets nothing.
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
        // Sessions name the build by its source, which the build script digests.
        assert!(
            manifest.contains("[build-dependencies]\n# Digests") && build.contains("emit()"),
            "{manifest}{build}"
        );
        assert_eq!(scene, SCENE);
        assert!(main.contains("App::new(\"probe\")"), "{main}");
        assert!(main.contains(".source(env!(\"GG_SOURCE\"))"), "{main}");
        // A start scene that fails to load ends the run with its error, not a panic.
        assert!(
            main.contains("cx.fail(error)") && !main.contains("panic!"),
            "{main}"
        );
        assert_eq!(refused.as_deref(), Some("gg.new"));
        assert!(empty);
    }

    /// Outside the engine's workspace: its own workspace, the engine by path, copies of its files.
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
        assert!(manifest.starts_with(WORKSPACE.split("{engine}").next().expect("a head")));
        assert!(manifest.ends_with(&MANIFEST.replace("{name}", "probe")));
        assert!(
            manifest.contains(concat!(
                "gg-engine = { path = \"../engine/crates/gg-engine\" }\n",
                "gg-api = { path = \"../engine/crates/gg-api\" }\n"
            )),
            "{manifest}"
        );
        // The engine's lints, which members take with `workspace = true`.
        for header in ["[workspace.lints.rust]", "[workspace.lints.clippy]"] {
            let engines = engine("Cargo.toml");
            assert_eq!(
                table(&manifest, header),
                table(&engines, header),
                "{header}"
            );
            assert!(!table(&manifest, header).is_empty(), "{header}");
        }
        // The engine's config but its alias, which works in the engine's workspace alone.
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

    /// `--engine`: a checkout, by path from a game a folder below or absolute, or a git repository
    /// at a revision. A checkout without the engine's crates is refused.
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

    /// Names in the engine's lock are taken: `--package` would name both.
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
