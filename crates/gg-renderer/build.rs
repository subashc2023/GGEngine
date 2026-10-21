//! Compiles the shaders in `shaders/` to SPIR-V with `slangc`, which the Vulkan SDK ships. The
//! `SLANGC` environment variable names it; otherwise it's the Vulkan SDK's, under `VULKAN_SDK`,
//! or the first on the path.

use std::env;
use std::path::PathBuf;
use std::process::Command;

const SHADERS: [&str; 2] = ["egui", "mesh"];

fn main() {
    println!("cargo::rerun-if-env-changed=SLANGC");
    println!("cargo::rerun-if-env-changed=VULKAN_SDK");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let slangc = slangc();
    for shader in SHADERS {
        let source = format!("shaders/{shader}.slang");
        println!("cargo::rerun-if-changed={source}");
        let output = Command::new(&slangc)
            .arg(&source)
            .args(["-target", "spirv", "-fvk-use-entrypoint-name", "-o"])
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

fn slangc() -> PathBuf {
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
