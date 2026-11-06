//! Compiles the shaders in `shaders/` to SPIR-V with `slangc`, which the Vulkan SDK ships. The
//! `SLANGC` environment variable names it; otherwise it's the Vulkan SDK's, under `VULKAN_SDK`,
//! or the first on the path. The crate compiles games' shaders as they're added with the same
//! one, which `GG_SLANGC` names to it.

use std::env;
use std::path::{self, Path, PathBuf};
use std::process::Command;

/// Those that draw, of a vertex and a fragment shader each.
const SHADERS: [&str; 13] = [
    "circle",
    "egui",
    "grid",
    "line",
    "model",
    "model_masked",
    "model_opaque",
    "outline",
    "sky",
    "sprite",
    "text",
    "tonemap",
    "view",
];

/// Those that compute, with their entry points.
const COMPUTE: [(&str, &[&str]); 5] = [
    ("clusters", &["bin_main"]),
    ("cull", &["choose_main", "offsets_main", "write_main"]),
    ("gather", &["gather_main"]),
    ("pyramid", &["depth_main", "reduce_main"]),
    (
        "prefilter",
        &[
            "cube_main",
            "downsample_main",
            "irradiance_main",
            "specular_main",
            "brdf_main",
        ],
    ),
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
    let drawing: &[(&str, &str)] = &[("vertex_main", "vertex"), ("fragment_main", "fragment")];
    for shader in SHADERS {
        compile(&slangc, &out, shader, shader, drawing);
    }
    // The masked model shader's shadows, cut out as it is, apart from it, which keeps to the
    // contract games' shaders keep, of two entry points.
    let shadow = &[("vertex_main", "vertex"), ("shadow_main", "fragment")];
    compile(&slangc, &out, "model_masked", "model_masked_shadow", shadow);
    for (shader, entries) in COMPUTE {
        let entries: Vec<_> = (entries.iter()).map(|&entry| (entry, "compute")).collect();
        compile(&slangc, &out, shader, shader, &entries);
    }
}

/// Compiles `shaders/<shader>.slang` with `slangc` into `<spirv>.spv` in `out`, of `entries`, each
/// an entry point and its stage.
fn compile(slangc: &Path, out: &Path, shader: &str, spirv: &str, entries: &[(&str, &str)]) {
    let source = format!("shaders/{shader}.slang");
    let mut command = Command::new(slangc);
    command
        .arg(&source)
        // As `compile` in src/shader/mod.rs compiles games' shaders, which says why.
        .args(["-target", "spirv", "-fvk-use-entrypoint-name"])
        .arg("-O0")
        .arg("-matrix-layout-column-major")
        .args(["-warnings-as-errors", "38006"])
        .args(["-warnings-disable", "39001"]);
    for (entry, stage) in entries {
        command.args(["-entry", entry, "-stage", stage]);
    }
    let output = command
        .arg("-o")
        .arg(out.join(format!("{spirv}.spv")))
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
