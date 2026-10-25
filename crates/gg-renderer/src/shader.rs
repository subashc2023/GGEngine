//! Shaders: Slang files compiled to SPIR-V with slangc, games' as they're added and the engine's
//! own as it builds, which draw meshes of one vertex type in the game's view.

use std::fmt;
use std::marker::PhantomData;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use bytemuck::NoUninit;
use gg_core::{Error, Result};
use gg_gpu::Blend;
use tracing::{debug, warn};

use crate::sprite::{self, Corner, Params};

/// The slangc that compiled the renderer's own shaders, as the build found it.
const SLANGC: &str = env!("GG_SLANGC");

/// A shader that [`Shaders::add`] handed out, which draws meshes of vertex type `V` and takes
/// parameters of type `P` with each draw.
pub struct Shader<V, P = ()> {
    pub(crate) index: u32,
    types: PhantomData<fn() -> (V, P)>,
}

// By hand, so that they don't need `V` and `P` to be `Clone` or `Debug` too.
impl<V, P> Clone for Shader<V, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V, P> Copy for Shader<V, P> {}

impl<V, P> Shader<V, P> {
    pub(crate) const fn new(index: u32) -> Self {
        Self {
            index,
            types: PhantomData,
        }
    }
}

impl<V, P> fmt::Debug for Shader<V, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Shader").field(&self.index).finish()
    }
}

/// The shaders a renderer draws meshes with, the first of them the engine's sprite shader, which
/// the build compiled. It compiles each one added as it's added and hands out its handle, and
/// keeps its SPIR-V until [`Uploads::take`](crate::Uploads::take) takes it, for a renderer to make
/// its pipeline the next time it renders. A renderer draws with the shaders of one `Shaders` only.
pub struct Shaders {
    /// Those added since they were last taken, in order.
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
    /// The size of the parameters it takes.
    pub(crate) params_size: usize,
    pub(crate) blend: Blend,
}

impl Shaders {
    /// Compiles the Slang shader at `path`, which draws meshes of vertex type `V` and takes
    /// parameters of type `P` with each draw, such as a color: `()` for none. `blend` says how its
    /// color goes over what's drawn before it: [`Blend::Alpha`] blends by the alpha it returns,
    /// not multiplied into the color. `name` names it in errors and GPU captures.
    ///
    /// Vulkan checks nothing about a shader, so the engine trusts it to keep to this contract,
    /// which validation checks in debug builds:
    ///
    /// - Its entry points are `vertex_main`, a vertex shader, and `fragment_main`, a fragment
    ///   shader that returns one `float4`: the color, in linear RGBA.
    /// - Each run of consecutive draws of one mesh with it is one instanced draw, whose instances
    ///   are the draws. Its push constants are `struct { float4x4 clip_from_world; V* vertices;
    ///   float4x4* transforms; P* params; }`, the only 88 bytes set. The first is the camera's
    ///   view and projection, from the world to Vulkan's clip space; the second, the address of
    ///   the mesh's vertices; the third, of the instances' transforms, each from the mesh's own
    ///   space to the world's, for `mul(clip_from_world, mul(transforms[instance],
    ///   float4(position, 1.0)))`; the fourth, of the instances' parameters, one `P` each, back to
    ///   back, which a shader that takes none leaves out.
    /// - It reads only the vertex at `SV_VulkanVertexID` and the instance at
    ///   `SV_VulkanInstanceID`, not `SV_VertexID` or `SV_InstanceID`, which need a feature the
    ///   engine doesn't enable. A fragment shader that reads its instance's parameters gets the
    ///   instance from the vertex shader, `nointerpolation`.
    /// - It may sample the bindless set's `[[vk::binding(0, 0)]] Texture2D textures[]`, by a
    ///   [`Texture`](crate::Texture) in its parameters, a `uint`, with its
    ///   `[[vk::binding(1, 0)]] SamplerState samplers[]`, by a [`Sampler`](crate::Sampler)'s
    ///   index, a `uint` too, each through `NonUniformResourceIndex`, since the instances of one
    ///   draw can carry different ones. It uses no other resources.
    /// - Its structs for `V` and `P` have their fields, in order, of 32-bit scalars, vectors and
    ///   matrices: Slang lays out what a pointer points at as `#[repr(C)]` does, without padding.
    ///   glam's `Vec2`, `Vec3`, `Vec4`, `Quat`, `Mat3` and `Mat4` are `float2`, `float3`,
    ///   `float4`, `float4`, `float3x3` and `float4x4`; not `Vec3A` or `Mat3A`, whose columns
    ///   take 16 bytes to `float3`'s 12. The engine builds glam without SIMD, so each of them is
    ///   4-byte aligned, as Slang's are.
    ///
    /// Fails with `renderer.bad_shader` and slangc's diagnostics if it doesn't compile, and with
    /// `renderer.no_slangc` if slangc can't be run.
    pub fn add<V: NoUninit, P: NoUninit>(
        &mut self,
        name: &str,
        path: &Path,
        blend: Blend,
    ) -> Result<Shader<V, P>> {
        let spirv = compile(Path::new(SLANGC), name, path)?;
        Ok(self.push(name, spirv, blend))
    }

    /// Takes `spirv`, shader `name` compiled, which draws meshes of vertex type `V` and takes
    /// parameters of type `P`, and hands out its handle.
    fn push<V, P>(&mut self, name: &str, spirv: Vec<u8>, blend: Blend) -> Shader<V, P> {
        let index = self.count;
        self.count += 1;
        self.added.push(Added {
            index,
            name: name.to_owned(),
            spirv,
            vertex_size: size_of::<V>(),
            params_size: size_of::<P>(),
            blend,
        });
        Shader::new(index)
    }
}

impl Default for Shaders {
    fn default() -> Self {
        let mut shaders = Self {
            added: Vec::new(),
            count: 0,
        };
        let shader = shaders.push::<Corner, Params>("sprite", sprite::SPIRV.to_vec(), Blend::Alpha);
        assert_eq!(
            shader.index,
            sprite::SHADER.index,
            "the sprite shader comes first"
        );
        shaders
    }
}

/// Compiles the shader `name` at `path` to SPIR-V with `slangc`.
fn compile(slangc: &Path, name: &str, path: &Path) -> Result<Vec<u8>> {
    let zone = gg_core::zone!();
    zone.text(path.display());
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
                .add::<[f32; 3], ()>("broken", &path, Blend::Opaque)
                .expect_err("a shader that doesn't compile");
            assert_eq!(error.code(), "renderer.bad_shader", "{file}");
            let message = error.to_string();
            assert!(
                message.contains("\"broken\"") && message.contains(&path.display().to_string()),
                "{message}"
            );
        }
        let mut flat = || {
            shaders.add::<[f32; 3], [f32; 4]>("flat", &fixture("flat_color.slang"), Blend::Opaque)
        };
        // After the engine's sprite shader.
        assert_eq!(flat().map(|shader| shader.index).ok(), Some(1));
        assert_eq!(flat().map(|shader| shader.index).ok(), Some(2));
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
