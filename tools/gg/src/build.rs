//! Cargo, for `gg`: building a package and finding its executable, and the workspace's target
//! directory, where games write their session files.

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
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
    let failed = |what: String| Failure::new("gg.cargo", what);
    let output = Command::new(cargo())
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| failed(format!("can't run cargo: {error}")))?;
    if !output.status.success() {
        let message = "`cargo metadata` failed, as cargo said above; gg runs in a workspace";
        return Err(failed(message.to_owned()));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| failed(format!("`cargo metadata` gave what isn't JSON: {error}")))?;
    let target = metadata["target_directory"].as_str();
    let target =
        target.ok_or_else(|| failed("`cargo metadata` named no target directory".to_owned()))?;
    Ok(PathBuf::from(target))
}
