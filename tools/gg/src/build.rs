//! Cargo, for `gg`: building a package and finding its executable; the workspace's target
//! directory, where games write their session files; and what `gg run`'s patches need of a package.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gg_api::Failure;
use serde_json::Value;

/// The cargo to run: the one running `gg`, as `cargo gg` does, or else the first on the `PATH`.
fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Builds package `package`, in the release profile if `release`, through cargo, whose messages
/// about the code go to stderr as cargo renders them; and returns its executable.
///
/// Fails with `gg.build` if cargo can't run, the build fails, or it makes no executable.
pub(crate) fn build(package: &str, release: bool) -> Result<PathBuf, Failure> {
    let mut command = Command::new(cargo());
    command.args([
        "build",
        "--package",
        package,
        "--message-format=json-render-diagnostics",
    ]);
    if release {
        command.arg("--release");
    }
    let failed = |what: String| Failure::new("gg.build", what);
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| failed(format!("can't run cargo: {error}")))?;
    let mut exe = None;
    let messages = BufReader::new(child.stdout.take().expect("piped"));
    for line in messages.lines().map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message["reason"] == "compiler-artifact"
            && let Some(path) = message["executable"].as_str()
        {
            exe = Some(PathBuf::from(path));
        }
    }
    let status = child
        .wait()
        .map_err(|error| failed(format!("cargo didn't finish: {error}")))?;
    if !status.success() {
        return Err(failed(format!(
            "`cargo build --package {package}` failed, as cargo said above"
        )));
    }
    exe.ok_or_else(|| failed(format!("package `{package}` has no executable")))
}

/// The directory cargo builds the workspace into, as `cargo metadata` says.
///
/// Fails with `gg.cargo` if cargo can't say, as outside a workspace.
pub(crate) fn target_dir() -> Result<PathBuf, Failure> {
    target_of(&metadata(false)?)
}

/// What `cargo metadata` says of the workspace, and with `deps`, of the packages it depends on and
/// how they resolve.
///
/// Fails with `gg.cargo` if cargo can't say, as outside a workspace.
fn metadata(deps: bool) -> Result<Value, Failure> {
    let failed = |what: String| Failure::new("gg.cargo", what);
    let mut command = Command::new(cargo());
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

/// A package of the workspace, as `gg run`'s patches need it.
pub(crate) struct Package {
    /// The binary it runs, as `cargo run` would pick it: `gg-sandbox` for the sandbox.
    pub(crate) bin: String,
    /// Its manifest.
    pub(crate) manifest: PathBuf,
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

/// Package `name` of the workspace, as `gg run`'s patches need it.
///
/// Fails with `gg.cargo` if cargo can't say, and as [`package_in`] does.
pub(crate) fn package(name: &str) -> Result<Package, Failure> {
    package_in(&metadata(true)?, name)
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

    #[test]
    fn a_package_is_its_binary_its_modules_and_the_workspace_packages_it_builds_with() {
        let game = package_in(&workspace(), "game").expect("the game");
        assert_eq!(game.bin, "gg-game");
        assert_eq!(game.manifest, Path::new("/w/game/Cargo.toml"));
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
