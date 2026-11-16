//! Cargo, for `gg`: building a package, the target directory (where games write session files),
//! what `gg run` and `gg ship` need of a package, and whether a project's game is the package
//! beside its manifest. Each runs cargo in a given directory, else `gg`'s own.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gg_api::Failure;
use serde_json::Value;

use crate::{Scope, dx};

/// Cargo, run in `dir`, or `gg`'s own: the one running `gg` (`cargo gg`), else the first on `PATH`.
pub(crate) fn cargo(dir: Option<&Path>) -> Command {
    let mut command =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command
}

/// What a build made: its executable, and `GG_SOURCE` if its build script gives one (gg-api's
/// `build`).
pub(crate) struct Built {
    pub(crate) exe: PathBuf,
    pub(crate) source: Option<String>,
}

/// Builds `package` in `dir` with `args` (`--release`...), showing rustc's messages on stderr.
///
/// # Errors
///
/// - `gg.build`: cargo can't run, the build fails (with rustc's errors) or makes no executable.
pub(crate) fn build(package: &str, args: &[&str], dir: Option<&Path>) -> Result<Built, Failure> {
    let mut command = cargo(dir);
    command.args(["build", "--package", package]).args(args);
    let format = if io::stderr().is_terminal() {
        "--message-format=json-diagnostic-rendered-ansi"
    } else {
        "--message-format=json"
    };
    command.arg(format);
    let failed = |what: String| Failure::new("gg.build", what);
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| failed(format!("can't run cargo: {error}")))?;
    let messages = BufReader::new(child.stdout.take().expect("piped"));
    let said = read_messages(messages.lines().map_while(Result::ok), |rendered| {
        let _ = io::stderr().lock().write_all(rendered.as_bytes());
    });
    let status = child
        .wait()
        .map_err(|error| failed(format!("cargo didn't finish: {error}")))?;
    if !status.success() {
        let args = args.iter().map(|arg| format!(" {arg}")).collect::<String>();
        let what = format!("cargo build --package {package}{args}");
        return Err(failed(if said.errors.is_empty() {
            format!("`{what}` failed, as cargo said above")
        } else {
            format!("`{what}` failed:\n\n{}", listed_errors(&said.errors))
        }));
    }
    let exe = (said.exe).ok_or_else(|| failed(format!("package `{package}` has no executable")))?;
    Ok(Built {
        exe,
        source: said.source,
    })
}

/// How many errors a failure gives; an early one often causes the rest, which are counted.
const MOST_ERRORS: usize = 5;

/// The most bytes of each error a failure gives.
const MOST_ERROR: usize = 2_000;

/// `text` cut to `most` bytes at a character boundary, saying so: a note's message is bounded.
pub(crate) fn fit(text: &str, most: usize) -> String {
    const CUT: &str = "\n\n(cut short: `gg run`'s output holds the rest)";
    if text.len() <= most {
        return text.to_owned();
    }
    let end = text.floor_char_boundary(most.saturating_sub(CUT.len()));
    format!("{}{CUT}", &text[..end])
}

/// Whether plain `error` is rustc's summary (`error: aborting due to 2 previous errors`).
pub(crate) fn summary(error: &str) -> bool {
    error.starts_with("error: aborting due to")
}

/// The first [`MOST_ERRORS`] of `errors`, each [`fit`] to [`MOST_ERROR`], then a count of the
/// rest: `gg run` repeats it with each call while no game answers.
pub(crate) fn listed_errors(errors: &[String]) -> String {
    let shown = errors
        .iter()
        .take(MOST_ERRORS)
        .map(|error| fit(error, MOST_ERROR));
    let mut listed = shown.collect::<Vec<_>>().join("\n\n");
    match errors.len().saturating_sub(MOST_ERRORS) {
        0 => {}
        1 => listed.push_str("\n\n(and 1 more error)"),
        more => listed.push_str(&format!("\n\n(and {more} more errors)")),
    }
    listed
}

/// What a build said: the last executable; `GG_SOURCE`, whether or not cargo ran the build script;
/// rustc's errors, plain, each once.
#[derive(Debug, Default, PartialEq)]
struct Said {
    exe: Option<PathBuf>,
    source: Option<String>,
    errors: Vec<String>,
}

/// Reads cargo's JSON `lines`, giving `show` each rustc message as rendered.
fn read_messages(lines: impl Iterator<Item = String>, mut show: impl FnMut(&str)) -> Said {
    let mut said = Said::default();
    for line in lines {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match message["reason"].as_str() {
            Some("compiler-artifact") => {
                if let Some(path) = message["executable"].as_str() {
                    said.exe = Some(PathBuf::from(path));
                }
            }
            Some("build-script-executed") => {
                let source = (list(&message["env"]).iter())
                    .find(|pair| pair[0] == "GG_SOURCE")
                    .and_then(|pair| pair[1].as_str());
                if let Some(source) = source {
                    said.source = Some(source.to_owned());
                }
            }
            Some("compiler-message") => {
                let diagnostic = &message["message"];
                let Some(rendered) = diagnostic["rendered"].as_str() else {
                    continue;
                };
                show(rendered);
                let level = diagnostic["level"].as_str().unwrap_or_default();
                let error = dx::plain(rendered);
                if level.starts_with("error") && !summary(&error) && !said.errors.contains(&error) {
                    said.errors.push(error);
                }
            }
            _ => {}
        }
    }
    said
}

/// The directory cargo builds the workspace into.
///
/// # Errors
///
/// - `gg.cargo`: cargo can't say, as outside a workspace.
pub(crate) fn target_dir(dir: Option<&Path>) -> Result<PathBuf, Failure> {
    target_of(&metadata(false, dir)?)
}

/// `cargo metadata` in `dir`: the workspace and, with `deps`, its dependencies and their
/// resolution.
///
/// # Errors
///
/// - `gg.cargo`: cargo can't say, as outside a workspace.
pub(crate) fn metadata(deps: bool, dir: Option<&Path>) -> Result<Value, Failure> {
    let failed = |what: String| Failure::new("gg.cargo", what);
    let mut command = cargo(dir);
    command.args(["metadata", "--format-version", "1"]);
    if !deps {
        command.arg("--no-deps");
    }
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| failed(format!("can't run cargo: {error}")))?;
    if !output.status.success() {
        let message = "`cargo metadata` failed, as cargo said above; gg runs in a workspace";
        return Err(failed(message.to_owned()));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| failed(format!("`cargo metadata` gave what isn't JSON: {error}")))
}

/// # Errors
///
/// - `gg.cargo`: `metadata` names no target directory.
fn target_of(metadata: &Value) -> Result<PathBuf, Failure> {
    let target = metadata["target_directory"]
        .as_str()
        .ok_or_else(|| Failure::new("gg.cargo", "`cargo metadata` named no target directory"))?;
    Ok(PathBuf::from(target))
}

/// A package of the workspace, as `gg run`'s patches and `gg ship` need it.
pub(crate) struct Package {
    /// The binary `cargo run` would pick: `gg-sandbox` for the sandbox.
    pub(crate) bin: String,
    pub(crate) manifest: PathBuf,
    /// The manifest's version: `0.1.0`.
    pub(crate) version: String,
    /// The directory of the binary's root module: `apps/sandbox/src`.
    pub(crate) modules: PathBuf,
    pub(crate) build_script: Option<PathBuf>,
    /// Whether it has a library beside its binary.
    pub(crate) library: bool,
    /// The directories of the workspace packages its binary depends on, except through dev
    /// dependencies.
    pub(crate) deps: Vec<PathBuf>,
    /// The release of subsecond its binary depends on, if any.
    pub(crate) subsecond: Option<String>,
    /// The engine checkout its binary builds with: two directories above gg-engine's manifest
    /// (its crates are in `crates/`), so the engine's workspace root or a checkout outside it, by
    /// path or cargo's of a git revision.
    pub(crate) engine: Option<PathBuf>,
    /// The workspace's root directory, and the directory cargo builds it into.
    pub(crate) root: PathBuf,
    pub(crate) target: PathBuf,
}

/// Package `name` of the workspace.
///
/// # Errors
///
/// - `gg.cargo`: cargo can't say.
/// - As [`package_in`] does.
pub(crate) fn package(name: &str, dir: Option<&Path>) -> Result<Package, Failure> {
    package_in(&metadata(true, dir)?, name)
}

/// Checks that `scope`'s game is the package beside the project's manifest: a copied manifest
/// fails.
///
/// # Errors
///
/// - `gg.project`: it isn't, naming both.
/// - `gg.cargo`: cargo can't say.
pub(crate) fn check_project(scope: &Scope) -> Result<(), Failure> {
    beside(&metadata(false, Some(scope.dir()))?, scope)
}

/// [`check_project`] on `metadata`, comparing canonical directories, as the project's may be named
/// otherwise.
fn beside(metadata: &Value, scope: &Scope) -> Result<(), Failure> {
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let game = scope.game.as_str();
    let manifest = (list(&metadata["packages"]).iter())
        .find(|package| package["name"] == game)
        .and_then(|package| Some(PathBuf::from(package["manifest_path"].as_str()?)));
    let dir = manifest.as_deref().and_then(Path::parent).map(canonical);
    if dir == Some(canonical(scope.dir())) {
        return Ok(());
    }
    let found = match &manifest {
        Some(manifest) => format!(
            "that package's manifest is {}, not beside the project's",
            manifest.display()
        ),
        None => "the workspace has no such package".to_owned(),
    };
    let project = scope.manifest.display();
    let message = format!("the project {project} names its game `{game}`, but {found}");
    Err(Failure::new("gg.project", message))
}

/// Package `name` of `metadata`, which has the dependencies.
///
/// # Errors
///
/// - `gg.build`: the workspace lacks it, or it has no binary `cargo run` would pick (the only one,
///   or its `default-run`).
/// - `gg.cargo`: `metadata` names no target directory.
fn package_in(metadata: &Value, name: &str) -> Result<Package, Failure> {
    let failed = |what: String| Failure::new("gg.build", what);
    let path = |value: &Value| PathBuf::from(value.as_str().unwrap_or_default());
    let dir = |package: &Value| {
        let manifest = path(&package["manifest_path"]);
        manifest.parent().map(Path::to_owned).unwrap_or_default()
    };
    let members: HashSet<&str> = (list(&metadata["workspace_members"]).iter())
        .filter_map(Value::as_str)
        .collect();
    let member = |package: &Value| {
        package["id"]
            .as_str()
            .is_some_and(|id| members.contains(id))
    };
    let packages: HashMap<&str, &Value> = (list(&metadata["packages"]).iter())
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect();
    let package = (list(&metadata["packages"]).iter())
        .find(|package| package["name"] == name && member(package))
        .ok_or_else(|| failed(format!("there's no package `{name}` in the workspace")))?;
    let targets = list(&package["targets"]);
    let is = |target: &&Value, kind: &str| list(&target["kind"]).iter().any(|of| of == kind);
    let bins: Vec<&Value> = targets.iter().filter(|target| is(target, "bin")).collect();
    let bin = match package["default_run"].as_str() {
        Some(run) => bins.into_iter().find(|bin| bin["name"] == run),
        None if bins.len() == 1 => bins.first().copied(),
        None => None,
    };
    let bin = bin.ok_or_else(|| {
        failed(format!(
            "package `{name}` has no binary, or more than one and no `default-run` to pick one"
        ))
    })?;
    let build_script = targets.iter().find(|target| is(target, "custom-build"));
    let libraries = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];
    let library = (targets.iter()).any(|target| libraries.iter().any(|kind| is(&target, kind)));
    // What its binary builds with: all it reaches, except through dev dependencies.
    let nodes: HashMap<&str, &Value> = (list(&metadata["resolve"]["nodes"]).iter())
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect();
    let id = package["id"].as_str().unwrap_or_default();
    let (mut reached, mut reaching) = (HashSet::new(), vec![id]);
    while let Some(id) = reaching.pop() {
        if !reached.insert(id) {
            continue;
        }
        let deps = nodes
            .get(id)
            .map(|node| list(&node["deps"]))
            .unwrap_or_default();
        for dep in deps {
            let built = match dep["dep_kinds"].as_array() {
                Some(kinds) => kinds.iter().any(|of| of["kind"] != "dev"),
                None => true,
            };
            if built && let Some(pkg) = dep["pkg"].as_str() {
                reaching.push(pkg);
            }
        }
    }
    let reached: Vec<&Value> = (reached.into_iter())
        .filter_map(|id| packages.get(id).copied())
        .collect();
    let mut deps: Vec<PathBuf> = (reached.iter())
        .filter(|dep| member(dep) && dep["id"] != id)
        .map(|dep| dir(dep))
        .collect();
    deps.sort();
    let subsecond = (reached.iter())
        .find(|dep| dep["name"] == "subsecond")
        .and_then(|dep| dep["version"].as_str())
        .map(str::to_owned);
    let engine = (reached.iter())
        .find(|dep| dep["name"] == "gg-engine")
        .and_then(|engine| Some(dir(engine).parent()?.parent()?.to_owned()));
    let modules = path(&bin["src_path"]).parent().map(Path::to_owned);
    Ok(Package {
        bin: bin["name"].as_str().unwrap_or_default().to_owned(),
        manifest: path(&package["manifest_path"]),
        version: package["version"].as_str().unwrap_or_default().to_owned(),
        modules: modules.unwrap_or_default(),
        build_script: build_script.map(|script| path(&script["src_path"])),
        library,
        deps,
        subsecond,
        engine,
        root: path(&metadata["workspace_root"]),
        target: target_of(metadata)?,
    })
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A build's messages give its executable, source and errors, each once and plain; every
    /// rustc message, warnings too, is shown.
    #[test]
    fn a_builds_messages_give_its_executable_source_and_errors() {
        let error = |text: &str| json!({"reason": "compiler-message", "message": {"level": "error", "rendered": text}});
        let script = |env: Value| json!({"reason": "build-script-executed", "env": env});
        let lines = [
            script(json!([])),
            script(json!([["OTHER", "x"], ["GG_SOURCE", "5c7cd8abeeed5f44"]])),
            script(json!([["OTHER", "y"]])),
            json!({"reason": "compiler-message", "message": {"level": "warning", "rendered": "w"}}),
            error("\u{1b}[31merror[E0308]\u{1b}[0m: mismatched types\n"),
            error("\u{1b}[31merror[E0308]\u{1b}[0m: mismatched types\n"),
            error("error: aborting due to 1 previous error\n"),
            json!({"reason": "compiler-artifact", "executable": "target/debug/gg-shards"}),
            json!({"reason": "build-finished", "success": false}),
        ];
        let mut shown = Vec::new();
        let lines = lines
            .iter()
            .map(Value::to_string)
            .chain(["not JSON".to_owned()]);
        let built = read_messages(lines, |rendered| shown.push(rendered.to_owned()));
        assert_eq!(
            built,
            Said {
                exe: Some(PathBuf::from("target/debug/gg-shards")),
                source: Some("5c7cd8abeeed5f44".to_owned()),
                errors: vec!["error[E0308]: mismatched types".to_owned()],
            }
        );
        assert_eq!(shown.len(), 4);
    }

    /// A failure gives the first errors, long ones cut, and counts the rest.
    #[test]
    fn a_builds_errors_are_bounded() {
        let errors: Vec<String> = (0..8).map(|at| format!("error {at}")).collect();
        let listed = listed_errors(&errors);
        assert!(listed.starts_with("error 0\n\nerror 1"), "{listed}");
        assert!(
            listed.contains("error 4") && !listed.contains("error 5"),
            "{listed}"
        );
        assert!(listed.ends_with("(and 3 more errors)"), "{listed}");
        assert_eq!(listed_errors(&errors[..1]), "error 0");
        let long = listed_errors(&["x".repeat(10_000)]);
        assert!(long.len() <= MOST_ERROR && long.ends_with("holds the rest)"));
    }

    /// A game whose binary depends on an engine (with macros and subsecond) and whose tests
    /// depend on a tool; and a game with two binaries and a library.
    fn workspace() -> Value {
        fn target(kind: &str, name: &str, src: &str) -> Value {
            json!({"kind": [kind], "name": name, "src_path": src})
        }
        let package = |id: &str, manifest: &str, targets: Value| {
            json!({"id": id, "name": id, "version": "0.1.0", "manifest_path": manifest,
                "default_run": null, "targets": targets})
        };
        let dep = |pkg: &str, kind: Value| json!({"pkg": pkg, "dep_kinds": [{"kind": kind}]});
        json!({
            "workspace_root": "/w",
            "target_directory": "/w/target",
            "workspace_members": ["game", "engine", "macros", "tool", "pair"],
            "packages": [
                package("game", "/w/game/Cargo.toml", json!([
                    target("bin", "gg-game", "/w/game/src/main.rs"),
                    target("custom-build", "build-script-build", "/w/game/build.rs"),
                ])),
                package("engine", "/w/engine/Cargo.toml", json!([
                    target("lib", "engine", "/w/engine/src/lib.rs"),
                ])),
                package("macros", "/w/macros/Cargo.toml", json!([
                    target("proc-macro", "macros", "/w/macros/src/lib.rs"),
                ])),
                package("tool", "/w/tool/Cargo.toml", json!([
                    target("lib", "tool", "/w/tool/src/lib.rs"),
                ])),
                package("pair", "/w/pair/Cargo.toml", json!([
                    target("bin", "first", "/w/pair/src/first.rs"),
                    target("bin", "second", "/w/pair/src/second.rs"),
                    target("lib", "pair", "/w/pair/src/lib.rs"),
                ])),
                {"id": "subsecond 0.7.2", "name": "subsecond", "version": "0.7.2",
                    "manifest_path": "/r/subsecond/Cargo.toml", "targets": []},
            ],
            "resolve": {"nodes": [
                {"id": "game", "deps": [dep("engine", Value::Null), dep("tool", json!("dev"))]},
                {"id": "engine", "deps": [
                    dep("macros", json!("build")),
                    dep("subsecond 0.7.2", Value::Null),
                ]},
                {"id": "macros", "deps": []},
                {"id": "tool", "deps": []},
                {"id": "pair", "deps": []},
                {"id": "subsecond 0.7.2", "deps": []},
            ]},
        })
    }

    /// A project's game is the package beside its manifest, not a copied one's nor a missing one.
    #[test]
    fn a_projects_game_is_the_package_beside_it() {
        let scope = |game: &str| Scope {
            manifest: PathBuf::from("/w/game/project.ron"),
            game: game.to_owned(),
        };
        assert_eq!(beside(&workspace(), &scope("game")), Ok(()));
        for game in ["engine", "nothing"] {
            let refused = beside(&workspace(), &scope(game)).err();
            assert_eq!(
                refused.map(|failure| failure.code).as_deref(),
                Some("gg.project")
            );
        }
    }

    #[test]
    fn a_package_is_its_binary_its_modules_and_the_workspace_packages_it_builds_with() {
        let game = package_in(&workspace(), "game").expect("the game");
        assert_eq!(game.bin, "gg-game");
        assert_eq!(game.manifest, Path::new("/w/game/Cargo.toml"));
        assert_eq!(game.version, "0.1.0");
        assert_eq!(game.modules, Path::new("/w/game/src"));
        assert_eq!(
            game.build_script.as_deref(),
            Some(Path::new("/w/game/build.rs"))
        );
        assert!(!game.library);
        // Not the tool its tests use, nor subsecond (not a member).
        let deps = [Path::new("/w/engine"), Path::new("/w/macros")];
        assert_eq!(game.deps, deps);
        assert_eq!(game.subsecond.as_deref(), Some("0.7.2"));
        assert_eq!(game.root, Path::new("/w"));
        assert_eq!(game.target, Path::new("/w/target"));
        let engine = package_in(&workspace(), "engine")
            .err()
            .map(|error| error.code);
        assert_eq!(engine.as_deref(), Some("gg.build"), "no binary");
        let pair = package_in(&workspace(), "pair")
            .err()
            .map(|error| error.code);
        assert_eq!(pair.as_deref(), Some("gg.build"), "no binary to pick");
        let mut picked = workspace();
        picked["packages"][4]["default_run"] = json!("second");
        let pair = package_in(&picked, "pair").expect("the pair");
        assert_eq!((pair.bin.as_str(), pair.subsecond), ("second", None));
        assert!(pair.library);
        let none = package_in(&workspace(), "nothing")
            .err()
            .map(|error| error.code);
        assert_eq!(none.as_deref(), Some("gg.build"));
    }

    /// A binary's engine is the gg-engine checkout it builds with, inside or outside the
    /// workspace, never one only its tests use.
    #[test]
    fn a_packages_engine_is_the_checkout_of_the_gg_engine_its_binary_builds_with() {
        // The game's metadata plus gg-engine at `manifest`, a `kind` dependency of node `node`.
        let engine = |manifest: &str, node: usize, kind: Value| {
            let mut metadata = workspace();
            let id = "gg-engine 0.1.0";
            let package = json!({"id": id, "name": "gg-engine", "version": "0.1.0",
                "manifest_path": manifest, "targets": []});
            (metadata["packages"].as_array_mut().expect("packages")).push(package);
            let dep = json!({"pkg": id, "dep_kinds": [{"kind": kind}]});
            let deps = metadata["resolve"]["nodes"][node]["deps"].as_array_mut();
            deps.expect("deps").push(dep);
            package_in(&metadata, "game").expect("the game").engine
        };
        let at = |dir: &str| Some(PathBuf::from(dir));
        let checkout = "/c/git/checkouts/ggengine-1a2b/48f0cd0";
        let outside = engine(
            &format!("{checkout}/crates/gg-engine/Cargo.toml"),
            1,
            Value::Null,
        );
        assert_eq!(outside, at(checkout));
        let inside = engine("/w/crates/gg-engine/Cargo.toml", 0, Value::Null);
        assert_eq!(inside, at("/w"));
        assert_eq!(
            engine("/e/crates/gg-engine/Cargo.toml", 0, json!("dev")),
            None
        );
        let game = package_in(&workspace(), "game").expect("the game");
        assert_eq!(game.engine, None);
    }
}
