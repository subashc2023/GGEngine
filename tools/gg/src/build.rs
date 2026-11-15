//! Cargo, for `gg`: building a package and finding its executable; the workspace's target
//! directory, where games write their session files; what `gg run`'s patches and `gg ship` need of
//! a package; and whether a project's game is the package beside its manifest. Each runs cargo in a
//! directory it's given, a project's, where it finds the workspace, or else in `gg`'s own.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gg_api::Failure;
use serde_json::Value;

use crate::{Scope, dx};

/// Cargo, run in directory `dir`, or in `gg`'s own: the one running `gg`, as `cargo gg` does, or
/// else the first on the `PATH`.
pub(crate) fn cargo(dir: Option<&Path>) -> Command {
    let mut command =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command
}

/// What a build made: its executable, and the digest of the source a game's code is compiled from,
/// `GG_SOURCE`, which its build script gives, as gg-api's `build` says, if the build ran one that
/// does.
pub(crate) struct Built {
    pub(crate) exe: PathBuf,
    pub(crate) source: Option<String>,
}

/// Builds package `package`, with `args`, cargo's flags that pick its profile and features, as
/// `--release`, through cargo run in `dir`, whose messages about the code go to stderr as rustc
/// renders them, in color on a terminal; and returns what it made.
///
/// Fails with `gg.build` if cargo can't run, the build fails, with rustc's errors, or it makes no
/// executable.
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

/// How many of a build's errors its failure gives, the first, as rustc reports them: the rest, which
/// an error early on often causes, are counted.
const MOST_ERRORS: usize = 5;

/// How long each error a build's failure gives may be, in bytes.
const MOST_ERROR: usize = 2_000;

/// `text`, cut to at most `most` bytes at a character's boundary, saying so where it's cut, as a
/// note's message is bounded, and a build's errors may run past it.
pub(crate) fn fit(text: &str, most: usize) -> String {
    const CUT: &str = "\n\n(cut short: `gg run`'s output holds the rest)";
    if text.len() <= most {
        return text.to_owned();
    }
    let end = text.floor_char_boundary(most.saturating_sub(CUT.len()));
    format!("{}{CUT}", &text[..end])
}

/// Whether `error`, one of rustc's, plain, is its summary of the others, `error: aborting due to 2
/// previous errors`, which says nothing they don't.
pub(crate) fn summary(error: &str) -> bool {
    error.starts_with("error: aborting due to")
}

/// A build's `errors`, rustc's, plain, as its failure gives them: the first [`MOST_ERRORS`], each
/// cut to [`MOST_ERROR`] bytes, as [`fit`] cuts it, then how many more there are, so that a
/// failure, which `gg run` tells of with each call while no game answers, stays small.
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

/// What a build's messages, cargo's lines of JSON, said: the executable it made, the last; the
/// `GG_SOURCE` a build script gave, which cargo says of a script it ran and of one it didn't need
/// to; and rustc's errors, each once, as plain text.
#[derive(Debug, Default, PartialEq)]
struct Said {
    exe: Option<PathBuf>,
    source: Option<String>,
    errors: Vec<String>,
}

/// Reads `lines`, cargo's messages, giving `show` each of rustc's, as rustc rendered it.
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

/// The directory cargo builds the workspace into, as `cargo metadata`, run in `dir`, says.
///
/// Fails with `gg.cargo` if cargo can't say, as outside a workspace.
pub(crate) fn target_dir(dir: Option<&Path>) -> Result<PathBuf, Failure> {
    target_of(&metadata(false, dir)?)
}

/// What `cargo metadata`, run in `dir`, says of the workspace, and with `deps`, of the packages it
/// depends on and how they resolve.
///
/// Fails with `gg.cargo` if cargo can't say, as outside a workspace.
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

/// The target directory `metadata`, what `cargo metadata` said, names.
///
/// Fails with `gg.cargo` if it names none.
fn target_of(metadata: &Value) -> Result<PathBuf, Failure> {
    let target = metadata["target_directory"]
        .as_str()
        .ok_or_else(|| Failure::new("gg.cargo", "`cargo metadata` named no target directory"))?;
    Ok(PathBuf::from(target))
}

/// A package of the workspace, as `gg run`'s patches and `gg ship` need it.
pub(crate) struct Package {
    /// The binary it runs, as `cargo run` would pick it: `gg-sandbox` for the sandbox.
    pub(crate) bin: String,
    /// Its manifest.
    pub(crate) manifest: PathBuf,
    /// Its version, as its manifest gives it: `0.1.0`.
    pub(crate) version: String,
    /// The directory of its binary's root module, under which its modules are, as
    /// `apps/sandbox/src`.
    pub(crate) modules: PathBuf,
    /// Its build script, if it has one.
    pub(crate) build_script: Option<PathBuf>,
    /// Whether it has a library beside its binary, which is a crate of its own.
    pub(crate) library: bool,
    /// The directories of the workspace's packages its binary depends on, directly or not, but for
    /// those only its tests do.
    pub(crate) deps: Vec<PathBuf>,
    /// The release of subsecond its binary depends on, if it does.
    pub(crate) subsecond: Option<String>,
    /// The workspace's root directory, and the directory cargo builds it into.
    pub(crate) root: PathBuf,
    pub(crate) target: PathBuf,
}

/// Package `name` of the workspace, as cargo run in `dir` finds it, as `gg run`'s patches and
/// `gg ship` need it.
///
/// Fails with `gg.cargo` if cargo can't say, and as [`package_in`] does.
pub(crate) fn package(name: &str, dir: Option<&Path>) -> Result<Package, Failure> {
    package_in(&metadata(true, dir)?, name)
}

/// Checks that the game the project `scope` names is the package whose manifest is beside the
/// project's, as a manifest copied from another game's wouldn't be.
///
/// Fails with `gg.project` if it isn't, naming both, and with `gg.cargo` if cargo can't say.
pub(crate) fn check_project(scope: &Scope) -> Result<(), Failure> {
    beside(&metadata(false, Some(scope.dir()))?, scope)
}

/// As [`check_project`], with `metadata`, what `cargo metadata` said of the workspace, comparing
/// the directories canonical where they're there, as the project's may be named in another form.
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

/// Package `name`, as `metadata`, what `cargo metadata` said with the dependencies, has it.
///
/// Fails with `gg.build` if the workspace has no such package, or the package has no binary that
/// `cargo run` would pick: one, or the one its `default-run` names; and with `gg.cargo` if
/// `metadata` names no target directory.
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
    // The packages its binary builds with: those its dependencies reach, but for those only tests
    // and benchmarks do.
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
        root: path(&metadata["workspace_root"]),
        target: target_of(metadata)?,
    })
}

/// The array `value` is, or none if it isn't one.
fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A build's messages give its executable, its game's source and rustc's errors, each once and
    /// plain, every one of rustc's messages shown as rendered, warnings too.
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

    /// A failure gives a build's first errors, each cut short if it's long, and counts the rest.
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

    /// A workspace's metadata: a game whose binary depends on an engine, which builds with macros
    /// and depends on subsecond, and whose tests depend on a tool; and a game with two binaries and
    /// a library.
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

    /// A project's game is the package beside its manifest, which a copied manifest's isn't, nor
    /// one of a package the workspace hasn't.
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
        // Not the tool its tests use, nor subsecond, which isn't the workspace's.
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
}
