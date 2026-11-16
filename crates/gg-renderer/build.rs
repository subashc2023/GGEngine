//! Compiles `shaders/` to SPIR-V with `slangc`: `SLANGC` if set, else the Vulkan SDK's under
//! `VULKAN_SDK`, else the first on the path. The crate compiles games' shaders with the same one,
//! which it gets as `GG_SLANGC`.

use std::env;
use std::path::{self, Path, PathBuf};
use std::process::Command;

/// Shaders that draw, each a vertex and a fragment shader.
const SHADERS: [&str; 14] = [
    "circle",
    "egui",
    "grid",
    "line",
    "model",
    "model_masked",
    "model_opaque",
    "outline",
    "particles",
    "sky",
    "sprite",
    "text",
    "tonemap",
    "view",
];

/// Shaders that compute, with their entry points.
const COMPUTE: [(&str, &[&str]); 22] = [
    (
        "ao",
        &[
            "prefilter_main",
            "occlusion_main",
            "denoise_main",
            "denoise_last_main",
        ],
    ),
    (
        "atmosphere",
        &[
            "transmittance_main",
            "multiple_main",
            "sky_view_main",
            "aerial_main",
            "cube_main",
        ],
    ),
    (
        "bloom",
        &["first_main", "down_main", "up_main", "composite_main"],
    ),
    ("clusters", &["bin_main"]),
    ("cull", &["choose_main", "offsets_main", "write_main"]),
    (
        "depth_of_field",
        &[
            "downsample_main",
            "tiles_main",
            "dilate_main",
            "gather_main",
            "composite_main",
        ],
    ),
    ("emit", &["emit_main", "grow_main", "moved_main"]),
    (
        "exposure",
        &["clear_main", "histogram_main", "average_main"],
    ),
    ("fogging", &["fog_main"]),
    ("gather", &["gather_main"]),
    ("lens", &["lens_main"]),
    ("meshlets", &["meshlets_main"]),
    (
        "motion_blur",
        &[
            "velocity_main",
            "tiles_main",
            "neighbors_main",
            "gather_main",
        ],
    ),
    ("outline_distances", &["distances_main"]),
    ("pyramid", &["depth_main", "reduce_main"]),
    (
        "reflections",
        &[
            "depth_main",
            "reduce_main",
            "trace_main",
            "blur_main",
            "keep_main",
        ],
    ),
    ("show", &["show_main"]),
    ("skin", &["skin_main"]),
    ("smaa", &["edges_main", "weights_main", "blend_main"]),
    ("taa", &["resolve_main"]),
    ("visibility", &["shade_main", "normal_main"]),
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
    check_release(&slangc);
    // The whole directory, so a change to an included file recompiles too.
    println!("cargo::rerun-if-changed=shaders");
    let drawing: &[(&str, &str)] = &[("vertex_main", "vertex"), ("fragment_main", "fragment")];
    for shader in SHADERS {
        compile(&slangc, &out, shader, shader, drawing);
    }
    // The atmosphere's sky, drawn, apart from its tables' compute shaders.
    compile(&slangc, &out, "atmosphere", "atmosphere_sky", drawing);
    // The masked model shader's cut-out shadows, apart from it, which keeps to games' shaders'
    // contract of two entry points.
    let shadow = &[("vertex_main", "vertex"), ("shadow_main", "fragment")];
    compile(&slangc, &out, "model_masked", "model_masked_shadow", shadow);
    // The opaque and masked model shaders' meshlets, through vertex shaders or a mesh shader, each
    // apart, so a device without mesh shaders takes no mesh shader's SPIR-V. The mesh shader alone:
    // gg-gpu's `GraphicsDesc::mesh` says why.
    let meshlets = ("meshlet_vertex_main", "vertex");
    for name in ["model_opaque", "model_masked"] {
        let entries = &[meshlets, ("fragment_main", "fragment")];
        compile(&slangc, &out, name, &format!("{name}_meshlets"), entries);
        let mesh = &[("mesh_main", "mesh")];
        compile(&slangc, &out, name, &format!("{name}_mesh"), mesh);
        // Their meshlets into the visibility buffer likewise: the mesh shader alone, and its
        // fragment shader alone too, as it reads its primitive's ID, a capability only devices
        // drawing through mesh shaders enable.
        let entries = &[
            ("meshlet_visibility_main", "vertex"),
            ("visibility_main", "fragment"),
        ];
        compile(&slangc, &out, name, &format!("{name}_visibility"), entries);
        let mesh = &[("mesh_visibility_main", "mesh")];
        compile(
            &slangc,
            &out,
            name,
            &format!("{name}_visibility_mesh"),
            mesh,
        );
        let fragment = &[("visibility_primitive_main", "fragment")];
        let spirv = format!("{name}_visibility_primitive");
        compile(&slangc, &out, name, &spirv, fragment);
        // Their normals, for the view's reflections, apart, as the masked one's shadows are: by
        // index, and by meshlets through vertex shaders. Mesh shader pipelines take the first's
        // fragment shader.
        let normals = &[("vertex_main", "vertex"), ("normal_main", "fragment")];
        compile(&slangc, &out, name, &format!("{name}_normals"), normals);
        let normals = &[meshlets, ("normal_main", "fragment")];
        compile(
            &slangc,
            &out,
            name,
            &format!("{name}_meshlets_normals"),
            normals,
        );
    }
    let shadow = &[meshlets, ("shadow_main", "fragment")];
    let spirv = "model_masked_meshlets_shadow";
    compile(&slangc, &out, "model_masked", spirv, shadow);
    for (shader, entries) in COMPUTE {
        let entries: Vec<_> = (entries.iter()).map(|&entry| (entry, "compute")).collect();
        compile(&slangc, &out, shader, shader, &entries);
    }
}

/// Compiles `entries` of `shaders/<shader>.slang`, each an entry point and stage, into
/// `<spirv>.spv` in `out`.
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

/// Warns if `slangc` isn't the release CI pins, its action's `SLANG`: each release's SPIR-V
/// differs, so a shader test can pass under one and fail under another, as
/// `the_model_shaders_load_the_frame_whole` did under 2025.23.2 and 2026.20. Silent where either
/// release can't be read.
fn check_release(slangc: &Path) {
    const ACTION: &str = "../../.github/actions/slangc/action.yml";
    println!("cargo::rerun-if-changed={ACTION}");
    let Ok(action) = std::fs::read_to_string(ACTION) else {
        return;
    };
    let Some(pinned) = action.lines().find_map(|line| {
        let value = line.trim().strip_prefix("SLANG:")?.trim();
        Some(value.trim_matches('"').to_owned())
    }) else {
        return;
    };
    // slangc prints its release to stderr, as `2026.17-25-gc7954ebd1` or `2025.23.2`.
    let Ok(output) = Command::new(slangc).arg("-version").output() else {
        return;
    };
    let printed = String::from_utf8_lossy(&output.stderr);
    let release = printed.trim();
    if release.is_empty() {
        return;
    }
    let same = release
        .strip_prefix(&pinned)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '.']));
    if !same {
        println!(
            "cargo::warning=slangc {release}, at {}, isn't {pinned}, the release CI builds with: \
             shaders' tests may pass here and fail there. Set SLANGC to a slangc {pinned}",
            slangc.display()
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
