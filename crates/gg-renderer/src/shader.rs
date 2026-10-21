//! Shaders that games bring: Slang files, compiled to SPIR-V with slangc as they're added, which
//! draw meshes of one vertex type in the game's view.

use std::fmt;
use std::marker::PhantomData;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use bytemuck::NoUninit;
use gg_core::{Error, Result};
use tracing::{debug, warn};

/// The slangc that compiled the renderer's own shaders, as the build found it.
const SLANGC: &str = env!("GG_SLANGC");

/// A shader that [`Shaders::add`] handed out, which draws meshes of vertex type `V`.
pub struct Shader<V> {
    pub(crate) index: u32,
    vertex: PhantomData<fn() -> V>,
}

// By hand, so that they don't need `V` to be `Clone` or `Debug` too.
impl<V> Clone for Shader<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for Shader<V> {}

impl<V> fmt::Debug for Shader<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Shader").field(&self.index).finish()
    }
}

/// The shaders a renderer draws meshes with. It compiles each one as it's added and hands out its
/// handle, and keeps its SPIR-V until a renderer makes its pipeline, the next time it renders. A
/// renderer draws with the shaders of one `Shaders` only.
#[derive(Default)]
pub struct Shaders {
    /// Those added since a renderer last took them, in order.
    pub(crate) added: Vec<Added>,
    /// How many have been added.
    count: u32,
}

pub(crate) struct Added {
    pub(crate) index: u32,
    pub(crate) name: String,
    pub(crate) spirv: Vec<u8>,
    /// The size of the vertices it reads.
    pub(crate) vertex_size: usize,
}

impl Shaders {
    /// Compiles the Slang shader at `path`, which draws meshes of vertex type `V`. `name` names it
    /// in errors and GPU captures.
    ///
    /// Vulkan checks nothing about a shader, so the engine trusts it to keep to this contract,
    /// which validation checks in debug builds:
    ///
    /// - Its entry points are `vertex_main`, a vertex shader, and `fragment_main`, a fragment
    ///   shader that returns one `float4`: the color, in linear RGBA.
    /// - Its push constants are `struct { float4x4 clip_from_mesh; V* vertices; }`, the only 72
    ///   bytes set. The first is the camera's view and projection times the draw's transform,
    ///   from the mesh's own space to Vulkan's clip space, for `mul(clip_from_mesh,
    ///   float4(position, 1.0))`; the second, the address of the mesh's vertices. It uses no
    ///   other resources.
    /// - It reads only the vertex at `SV_VulkanVertexID`, not `SV_VertexID`, which needs a
    ///   feature the engine doesn't enable.
    /// - Its struct for `V` has `V`'s fields, in order, of 32-bit types: Slang lays out what a
    ///   pointer points at as `#[repr(C)]` does.
    ///
    /// Fails with `renderer.bad_shader` and slangc's diagnostics if it doesn't compile, and with
    /// `renderer.no_slangc` if slangc can't be run.
    pub fn add<V: NoUninit>(&mut self, name: &str, path: &Path) -> Result<Shader<V>> {
        let spirv = compile(Path::new(SLANGC), name, path)?;
        let index = self.count;
        self.count += 1;
        self.added.push(Added {
            index,
            name: name.to_owned(),
            spirv,
            vertex_size: size_of::<V>(),
        });
        Ok(Shader {
            index,
            vertex: PhantomData,
        })
    }
}

/// Compiles the shader `name` at `path` to SPIR-V with `slangc`.
fn compile(slangc: &Path, name: &str, path: &Path) -> Result<Vec<u8>> {
    let start = Instant::now();
    let output = Command::new(slangc)
        .arg(path)
        .args(["-target", "spirv", "-fvk-use-entrypoint-name"])
        // As glam lays matrices out. It's slangc's default too, but only for legacy reasons, as
        // Slang's docs have it: elsewhere the default is row-major.
        .arg("-matrix-layout-column-major")
        // A function marked for one stage but compiled as the other.
        .args(["-warnings-as-errors", "38006"])
        .args(["-entry", "vertex_main", "-stage", "vertex"])
        .args(["-entry", "fragment_main", "-stage", "fragment"])
        .output()
        .map_err(|error| {
            Error::new(
                "renderer.no_slangc",
                format!(
                    "can't run slangc, {}, to compile shader {name:?}; install the Vulkan SDK, or \
                     set SLANGC to slangc's path and rebuild",
                    slangc.display()
                ),
            )
            .with_source(error)
        })?;
    // In lines of their own, without the carriage returns slangc ends them with on Windows.
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    let diagnostics = diagnostics.lines().collect::<Vec<_>>().join("\n");
    let diagnostics = diagnostics.trim_end();
    if !output.status.success() {
        return Err(Error::new(
            "renderer.bad_shader",
            format!(
                "shader {name:?} at {} doesn't compile ({}):\n{diagnostics}",
                path.display(),
                output.status
            ),
        ));
    }
    if !diagnostics.is_empty() {
        warn!(
            code = "renderer.shader_warning",
            shader = name,
            path = %path.display(),
            "{diagnostics}"
        );
    }
    debug!(
        shader = name,
        path = %path.display(),
        ms = start.elapsed().as_millis() as u64,
        "Shader compiled"
    );
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture(file: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/shaders")
            .join(file)
    }

    #[test]
    fn a_shader_that_doesnt_compile_names_itself_and_takes_no_handle() {
        let mut shaders = Shaders::default();
        for file in ["no_fragment.slang", "missing.slang"] {
            let path = fixture(file);
            let error = shaders
                .add::<[f32; 3]>("broken", &path)
                .expect_err("a shader that doesn't compile");
            assert_eq!(error.code(), "renderer.bad_shader", "{file}");
            let message = error.to_string();
            assert!(
                message.contains("\"broken\"") && message.contains(&path.display().to_string()),
                "{message}"
            );
        }
        let mut flat = || shaders.add::<[f32; 3]>("flat", &fixture("flat_color.slang"));
        assert_eq!(flat().map(|shader| shader.index).ok(), Some(0));
        assert_eq!(flat().map(|shader| shader.index).ok(), Some(1));
    }

    #[test]
    fn a_missing_slangc_is_its_own_error() {
        let error = compile(
            Path::new("no-such-slangc"),
            "flat",
            &fixture("flat_color.slang"),
        )
        .expect_err("no slangc");
        assert_eq!(error.code(), "renderer.no_slangc");
    }
}
