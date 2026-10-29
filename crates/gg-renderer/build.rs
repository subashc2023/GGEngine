//! Compiles the shaders in `shaders/` to SPIR-V with `slangc`, which the Vulkan SDK ships. The
//! `SLANGC` environment variable names it; otherwise it's the Vulkan SDK's, under `VULKAN_SDK`,
//! or the first on the path. The crate compiles games' shaders as they're added with the same
//! one, which `GG_SLANGC` names to it.

use std::env;
use std::path::{self, PathBuf};
use std::process::Command;

const SHADERS: [&str; 9] = [
    "circle",
    "egui",
    "line",
    "model",
    "model_masked",
    "model_opaque",
    "outline",
    "sprite",
    "view",
];

fn main() {
    println!("cargo::rerun-if-env-changed=SLANGC");
    println!("cargo::rerun-if-env-changed=VULKAN_SDK");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let slangc = slangc();
    let Some(name) = slangc.to_str() else {
        panic!(
            "slangc's path, {}, isn't UTF-8; set SLANGC to one that is",
            slangc.display()
        );
    };
    println!("cargo::rustc-env=GG_SLANGC={name}");
    // The whole directory, so that a change to a file the shaders include compiles them again too.
    println!("cargo::rerun-if-changed=shaders");
    for shader in SHADERS {
        let source = format!("shaders/{shader}.slang");
        let output = Command::new(&slangc)
            .arg(&source)
            // As `compile` in src/shader/mod.rs compiles games' shaders, which says why.
            .args(["-target", "spirv", "-fvk-use-entrypoint-name"])
            .arg("-matrix-layout-column-major")
            .args(["-warnings-as-errors", "38006"])
            .args(["-entry", "vertex_main", "-stage", "vertex"])
            .args(["-entry", "fragment_main", "-stage", "fragment"])
            .arg("-o")
            .arg(out.join(format!("{shader}.spv")))
            .output()
            .unwrap_or_else(|error| {
                panic!(
                    "can't run {}: {error}; install the Vulkan SDK, or set SLANGC to slangc's path",
                    slangc.display()
                )
            });
        if !output.status.success() {
            panic!(
                "slangc failed on {source}:\n{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

/// slangc's path, absolute unless it's a bare name to look up on the path: the build runs in the
/// crate's directory, and games' shaders compile wherever they run.
fn slangc() -> PathBuf {
    let found = found();
    if found.components().count() > 1 {
        path::absolute(&found).unwrap_or_else(|error| {
            panic!(
                "can't make slangc's path, {}, absolute: {error}",
                found.display()
            )
        })
    } else {
        found
    }
}

fn found() -> PathBuf {
    if let Some(path) = env::var_os("SLANGC") {
        return path.into();
    }
    let exe = if cfg!(windows) {
        "slangc.exe"
    } else {
        "slangc"
    };
    if let Some(sdk) = env::var_os("VULKAN_SDK") {
        // The SDK's tools are in `Bin` on Windows and `bin` elsewhere.
        for dir in ["Bin", "bin"] {
            let path = PathBuf::from(&sdk).join(dir).join(exe);
            if path.exists() {
                return path;
            }
        }
    }
    PathBuf::from(exe)
}
