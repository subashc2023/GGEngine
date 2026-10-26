//! Shaders: Slang files compiled to SPIR-V with slangc, games' as they're added and the engine's
//! own as it builds, which draw meshes of one vertex type in the game's view.

use std::collections::HashSet;
use std::fmt;
use std::marker::PhantomData;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use bytemuck::NoUninit;
use gg_core::{Error, Result};
use gg_gpu::Blend;
use tracing::{debug, warn};

use crate::model::{self, AlphaMode, ModelVertex};
use crate::sprite::{self, Corner, Params};

/// The slangc that compiled the renderer's own shaders, as the build found it.
const SLANGC: &str = env!("GG_SLANGC");

/// A shader that [`Shaders::add`] handed out, which draws meshes of vertex type `V` and takes
/// parameters of type `P` with each draw.
pub struct Shader<V, P = ()> {
    pub(crate) index: u32,
    /// Whether it writes depth, as [`writes_depth`] says.
    pub(crate) opaque: bool,
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
    pub(crate) const fn new(index: u32, opaque: bool) -> Self {
        Self {
            index,
            opaque,
            types: PhantomData,
        }
    }
}

/// Whether a shader that blends as `blend` says writes its depth where it draws, as well as
/// testing it: an opaque one does, so that what's behind it, drawn after it, doesn't show; one
/// that blends doesn't, so that what's behind it still shows through it, as through its
/// transparent margins.
pub(crate) fn writes_depth(blend: Blend) -> bool {
    blend == Blend::Opaque
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
    /// The handles of the engine's model shaders, for opaque, masked and blended materials, once
    /// each has been asked for.
    models: [Option<u32>; 3],
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
    /// not multiplied into the color. Every shader tests the view's depth, drawing only where it's
    /// as near as what's drawn there or nearer; one that's [`Blend::Opaque`] writes its depth too,
    /// so that what's behind it doesn't show over it, while one that blends doesn't, so that what's
    /// behind it still shows through it. `name` names it in errors and GPU captures.
    ///
    /// Vulkan checks nothing about a shader, so the engine trusts it to keep to this contract:
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
    /// Validation, in debug builds, checks only what the pipeline shows it: its entry points, the
    /// features it needs and the resources it binds. It can't see the layouts of what the shader
    /// reads through addresses, its vertices and parameters, so a struct that doesn't match its
    /// type draws garbage without a message; nor whether an index that can differ across a draw
    /// goes through `NonUniformResourceIndex`.
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
    /// parameters of type `P`, with its sampled images decorated as [`decorate_non_uniform`]
    /// says, and hands out its handle.
    fn push<V, P>(&mut self, name: &str, spirv: Vec<u8>, blend: Blend) -> Shader<V, P> {
        let index = self.count;
        self.count += 1;
        self.added.push(Added {
            index,
            name: name.to_owned(),
            spirv: decorate_non_uniform(spirv),
            vertex_size: size_of::<V>(),
            params_size: size_of::<P>(),
            blend,
        });
        Shader::new(index, writes_depth(blend))
    }

    /// The engine's model shader for materials that cover what's behind them as `alpha` says,
    /// which the build compiled, added as a game's are the first time it's asked for: a renderer
    /// draws models with it, but makes no pipeline for it without them, and games' handles come
    /// where they would have.
    pub(crate) fn model(&mut self, alpha: AlphaMode) -> Shader<ModelVertex, model::Params> {
        let (slot, name, spirv, blend) = match alpha {
            AlphaMode::Opaque => (0, "model", model::OPAQUE_SPIRV, Blend::Opaque),
            AlphaMode::Mask { .. } => (1, "model masked", model::MASKED_SPIRV, Blend::Opaque),
            AlphaMode::Blend => (2, "model blended", model::BLENDED_SPIRV, Blend::Alpha),
        };
        if let Some(index) = self.models[slot] {
            return Shader::new(index, writes_depth(blend));
        }
        let shader = self.push(name, spirv.to_vec(), blend);
        self.models[slot] = Some(shader.index);
        shader
    }
}

impl Default for Shaders {
    fn default() -> Self {
        let mut shaders = Self {
            added: Vec::new(),
            count: 0,
            models: [None; 3],
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

/// SPIR-V's magic number, which opens a module, and what [`decorate_non_uniform`] reads and adds:
/// `OpDecorate`, `OpSampledImage` and the `NonUniform` decoration.
const MAGIC: u32 = 0x0723_0203;
const OP_DECORATE: u32 = 71;
const OP_SAMPLED_IMAGE: u32 = 86;
const NON_UNIFORM: u32 = 5300;

/// `spirv`, a shader slangc compiled, with each sampled image that it makes of an image or a
/// sampler decorated `NonUniform` decorated so too. Vulkan requires the decoration on the operand
/// an instruction reaches a resource through, which for a sample is the sampled image
/// (VUID-RuntimeSpirv-subgroupSize-10149), but slangc before Slang 2026.11, the Vulkan SDK
/// 1.4.335's among them, decorates only the loads of the image and the sampler
/// (shader-slang/slang#10525): a driver that reads the decoration where Vulkan puts it may sample
/// one instance's texture for every instance of a subgroup. This can go once the engine requires
/// a slangc that decorates it, Slang 2026.11's or later, such as the Vulkan SDK 1.4.363's. SPIR-V
/// it can't read stays as it is, for Vulkan to refuse.
fn decorate_non_uniform(spirv: Vec<u8>) -> Vec<u8> {
    let (words, []) = spirv.as_chunks::<4>() else {
        return spirv;
    };
    let mut words: Vec<u32> = words.iter().map(|&word| u32::from_le_bytes(word)).collect();
    if words.len() < 5 || words[0] != MAGIC {
        return spirv;
    }
    // The IDs decorated NonUniform; where the first of those decorations ends, which is where
    // more go; and each sampled image, with the image and the sampler it's made of.
    let mut non_uniform = HashSet::new();
    let mut end = None;
    let mut sampled = Vec::new();
    let mut at = 5;
    while at < words.len() {
        // Each instruction's first word is its length in words, then its opcode.
        let length = (words[at] >> 16) as usize;
        let Some(instruction) = words.get(at..at + length).filter(|_| length > 0) else {
            return spirv;
        };
        match (words[at] & 0xffff, instruction) {
            (OP_DECORATE, &[_, id, NON_UNIFORM]) => {
                non_uniform.insert(id);
                end.get_or_insert(at + length);
            }
            (OP_SAMPLED_IMAGE, &[_, _, id, image, sampler]) => sampled.push([id, image, sampler]),
            _ => {}
        }
        at += length;
    }
    let decorations: Vec<u32> = sampled
        .into_iter()
        .filter(|[id, image, sampler]| {
            !non_uniform.contains(id)
                && (non_uniform.contains(image) || non_uniform.contains(sampler))
        })
        .flat_map(|[id, ..]| [(3 << 16) | OP_DECORATE, id, NON_UNIFORM])
        .collect();
    let Some(end) = end.filter(|_| !decorations.is_empty()) else {
        return spirv;
    };
    words.splice(end..end, decorations);
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
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

    /// The IDs of the sampled images `spirv` makes, each with whether it's decorated NonUniform.
    fn sampled_images(spirv: &[u8]) -> Vec<(u32, bool)> {
        let words: Vec<u32> = (spirv.as_chunks::<4>().0.iter())
            .map(|&word| u32::from_le_bytes(word))
            .collect();
        let (mut non_uniform, mut sampled) = (HashSet::new(), Vec::new());
        let mut at = 5;
        while at < words.len() {
            let length = (words[at] >> 16) as usize;
            match (words[at] & 0xffff, &words[at..at + length]) {
                (OP_DECORATE, &[_, id, NON_UNIFORM]) => {
                    non_uniform.insert(id);
                }
                (OP_SAMPLED_IMAGE, &[_, _, id, ..]) => sampled.push(id),
                _ => {}
            }
            at += length;
        }
        let decorated = |id| (id, non_uniform.contains(&id));
        sampled.into_iter().map(decorated).collect()
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
    fn shaders_sample_through_the_bindless_set_non_uniformly() {
        let mut shaders = Shaders::default();
        for alpha in [
            AlphaMode::Opaque,
            AlphaMode::Mask { cutoff: 0.5 },
            AlphaMode::Blend,
        ] {
            shaders.model(alpha);
        }
        for file in ["textured.slang", "level.slang"] {
            (shaders.add::<[f32; 5], [u32; 3]>(file, &fixture(file), Blend::Alpha))
                .expect("a shader");
        }
        // The sprite shader, the three model shaders and the two games'.
        assert_eq!(shaders.added.len(), 6);
        for added in &shaders.added {
            let sampled = sampled_images(&added.spirv);
            assert!(
                !sampled.is_empty() && sampled.iter().all(|&(_, decorated)| decorated),
                "{}: {sampled:?}",
                added.name
            );
        }
    }

    #[test]
    fn sampled_images_of_non_uniform_images_or_samplers_are_non_uniform() {
        let module = |instructions: &[[u32; 3]], samples: &[[u32; 5]]| {
            let header = [MAGIC, 0x0001_0500, 0, 64, 0];
            let body = instructions
                .as_flattened()
                .iter()
                .chain(samples.as_flattened());
            let words = header.iter().chain(body);
            words
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<_>>()
        };
        let decorate = |id| [(3 << 16) | OP_DECORATE, id, NON_UNIFORM];
        let sample = |id, image, sampler| [(5 << 16) | OP_SAMPLED_IMAGE, 1, id, image, sampler];
        // Image 10 and sampler 21 are non-uniform, image 11 and sampler 20 aren't; sampled image
        // 33 is decorated already.
        let samples = [
            sample(30, 10, 20),
            sample(31, 11, 21),
            sample(32, 11, 20),
            sample(33, 10, 21),
        ];
        let spirv = module(&[decorate(10), decorate(21), decorate(33)], &samples);
        let decorated = decorate_non_uniform(spirv.clone());
        let sampled = [(30, true), (31, true), (32, false), (33, true)];
        assert_eq!(sampled_images(&decorated), sampled);
        // After the first NonUniform decoration, among the others.
        let expected = [
            decorate(10),
            decorate(30),
            decorate(31),
            decorate(21),
            decorate(33),
        ];
        assert_eq!(decorated, module(&expected, &samples));
        // What it can't read stays as it is: an instruction cut short, an instruction of no words,
        // bytes that aren't whole words, and what isn't SPIR-V.
        let mut empty = spirv.clone();
        empty.splice(20..20, [0; 4]);
        let unreadable = [
            spirv[..spirv.len() - 4].to_vec(),
            empty,
            spirv[..spirv.len() - 1].to_vec(),
            b"not SPIR-V but words".to_vec(),
        ];
        for bytes in unreadable {
            assert_eq!(decorate_non_uniform(bytes.clone()), bytes);
        }
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
