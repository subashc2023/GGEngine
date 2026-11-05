//! Shaders: Slang files compiled to SPIR-V with slangc, games' as they're added and the engine's
//! own as it builds, which draw meshes of one vertex type in the game's view.

mod spirv;

use std::any::type_name;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime};

use bytemuck::NoUninit;
use gg_core::math::Vec2;
use gg_core::{Error, Result};
use gg_gpu::Blend;
use tracing::{debug, warn};

use crate::model::{self, AlphaMode, ModelVertex};
use crate::sprite::{self, Corner, Params};
use crate::{circle, grid, line, text};

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
/// its pipeline the next time it renders; a game's shader compiled again replaces it the same way,
/// and a shader removed goes with them, in order. A renderer draws with the shaders of one
/// `Shaders` only.
pub struct Shaders {
    /// Those added, replaced or removed since they were last taken, in order.
    pub(crate) changes: Vec<Change>,
    /// How many have been added.
    count: u32,
    /// Those removed, whose handles are never given out again.
    removed: HashSet<u32>,
    /// The handles of the engine's shaders that are added the first time they're asked for, once
    /// each has been: its model shaders, for opaque, masked and blended materials, then its circle
    /// shader, its line shader, its grid shader and its text shader.
    lazy: [Option<u32>; 7],
    /// The games' shaders, in the order they were added, less those removed.
    sources: Vec<ShaderSource>,
}

/// A change to a renderer's shaders, which [`Uploads::take`](crate::Uploads::take) carries to it.
pub(crate) enum Change {
    /// A shader added, or one compiled again in place of one.
    Added(Added),
    /// A shader removed, by its index.
    Removed(u32),
}

pub(crate) struct Added {
    pub(crate) index: u32,
    /// Whether it's a shader compiled again, in place of one a renderer has, rather than one
    /// added, which comes after those a renderer has.
    pub(crate) replacement: bool,
    pub(crate) name: String,
    pub(crate) spirv: Vec<u8>,
    /// The size of the vertices it reads.
    pub(crate) vertex_size: usize,
    /// The size of the parameters it takes.
    pub(crate) params_size: usize,
    pub(crate) blend: Blend,
    /// What draws the shadows of what it draws, if it writes depth and they're cut out as it is:
    /// a module of its vertex shader and `shadow_main`, a fragment shader that discards where it
    /// does, as the engine's masked model shader has. Those of the others that write depth are of
    /// whole triangles, drawn by their vertex shaders alone.
    pub(crate) shadow: Option<Vec<u8>>,
}

/// What a shader draws with: the sizes and names of its vertex type and its type of parameters,
/// which the contract's check compares its structs for them with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Types {
    vertex_size: usize,
    vertex_name: &'static str,
    params_size: usize,
    params_name: &'static str,
}

impl Types {
    fn of<V, P>() -> Self {
        Self {
            vertex_size: size_of::<V>(),
            vertex_name: type_name::<V>(),
            params_size: size_of::<P>(),
            params_name: type_name::<P>(),
        }
    }

    fn vertex(&self) -> (usize, &'static str) {
        (self.vertex_size, self.vertex_name)
    }

    fn params(&self) -> (usize, &'static str) {
        (self.params_size, self.params_name)
    }
}

impl Shaders {
    /// How many it has handed out, the engine's sprite shader among them, and those of its own it
    /// added as they were first asked for: the index of the next.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Compiles the Slang shader at `path`, which draws meshes of vertex type `V` and takes
    /// parameters of type `P` with each draw, such as a color: `()` for none. `blend` says how its
    /// color goes over what's drawn before it: [`Blend::Alpha`] blends by the alpha it returns, not
    /// multiplied into the color, and [`Blend::PremultipliedAlpha`] by the alpha it returns,
    /// multiplied into the color already. Every shader tests the view's depth, drawing only where
    /// it's as near as what's drawn there or nearer; one that's [`Blend::Opaque`] writes its depth
    /// too, so that what's behind it doesn't show over it, while one that blends doesn't, so that
    /// what's behind it still shows through it. What one that's opaque draws casts shadows, from
    /// the light that casts them, of whole triangles, as its vertex shader places them, pushed a
    /// shadow map's matrix from the world to its clip space and the map's size in place of the
    /// view's, and the frame's data without the sky or the shadows. `name` names it in errors and
    /// GPU captures.
    ///
    /// Vulkan checks nothing about a shader, so the engine checks, in the SPIR-V slangc writes,
    /// that it keeps to this contract:
    ///
    /// - Its entry points are `vertex_main`, a vertex shader, and `fragment_main`, a fragment
    ///   shader that returns a struct of two outputs, and nothing else: the color, a
    ///   `float4 : SV_Target0`, in linear RGBA, which may be brighter than 1, as the view's colors
    ///   are of high dynamic range; and the draw's ID, a `uint2 : SV_Target1`, which the view
    ///   keeps where the draw shows, for [picks](crate::Pick) to read back: its instance's, which
    ///   the vertex shader reads from `ids[instance]` and hands on `nointerpolation`, or 0 where
    ///   it draws no entity. A shader that blends should discard
    ///   what's fully transparent, after it samples, as the engine's do: blending it changes no
    ///   pixel, but its ID would hide what's behind it from picks.
    /// - Each run of consecutive draws of one mesh with it is one instanced draw, whose instances
    ///   are the draws. Its push constants are `struct { float4x4 clip_from_world; V* vertices;
    ///   float4x4* transforms; uint2* ids; P* params; float2 view; Frame* frame; }`, the only 112
    ///   bytes set. The first is the camera's view and projection, from the world to Vulkan's clip
    ///   space; the second, the address of the mesh's vertices; the third, of the instances'
    ///   transforms, each from the mesh's own space to the world's, for `mul(clip_from_world,
    ///   mul(transforms[instance], float4(position, 1.0)))`; the fourth, of the instances' IDs,
    ///   each the low and high halves of a 64-bit number; the fifth, of the instances' parameters,
    ///   one `P` each, back to back; the sixth, the view's size in pixels, across then down, for
    ///   what's sized on screen, as the engine's lines are; the seventh, of the frame's data,
    ///   `struct Frame { float3 camera; float time; float exposure; uint light_count; Light*
    ///   lights; Material* materials; float4 eye; uint sky; uint irradiance; uint specular; uint
    ///   specular_levels; uint brdf; float sky_luminance; Shadows shadows; }`, where the view sees
    ///   from in the world, the time it shows, what lit shading multiplies light by, and the
    ///   frame's lights, as [`RenderData`](crate::RenderData) gives them, `Light` as
    ///   `shaders/mesh.slang` declares it; the address of the table of models' materials; where
    ///   shading sees from, the camera's position and 1, or for an orthographic projection the way
    ///   back toward it and 0, so that the way to the eye from `p` is `eye.xyz - p * eye.w`; the
    ///   sky, as `shaders/mesh.slang` says, its texture, its prefiltered light, cubes, the split
    ///   sum's table and its luminance, `specular_levels` 0 in a frame without one; and the
    ///   shadows of the light that casts them, their cascades' shadow maps among them, `Shadows`
    ///   as `shaders/mesh.slang` declares it, its `light` 0xFFFFFFFF in a frame without them; or a
    ///   struct of as many of its first fields: the engine adds fields at its end. A shader that
    ///   takes no parameters leaves out the fifth, and so those after it. A shader may end the
    ///   struct after the last of them it reads, and one that reads none has none.
    /// - It reads only the vertex at `SV_VulkanVertexID` and the instance at
    ///   `SV_VulkanInstanceID`, not `SV_VertexID` or `SV_InstanceID`, which need a feature the
    ///   engine doesn't enable, nor vertex attributes. A fragment shader that reads its instance's
    ///   parameters gets the instance from the vertex shader, `nointerpolation`. It writes nothing
    ///   through the addresses, which needs features the engine doesn't enable too.
    /// - It may sample the bindless set's `[[vk::binding(0, 0)]] Texture2D textures[]`, by a
    ///   [`Texture`](crate::Texture) in its parameters, a `uint`, with its
    ///   `[[vk::binding(1, 0)]] SamplerState samplers[]`, by a [`Sampler`](crate::Sampler)'s
    ///   index, a `uint` too, each through `NonUniformResourceIndex`, since the instances of one
    ///   draw can carry different ones; the cubes the frame's data names, through
    ///   `[[vk::binding(0, 0)]] TextureCube cubes[]`, the same binding; and the shadow maps the
    ///   frame's data names, by comparison alone, as `SampleCmp` samples, through
    ///   `[[vk::binding(1, 0)]] SamplerComparisonState comparisons[]`, the samplers' binding, at
    ///   [`COMPARISON_SAMPLER`](gg_gpu::COMPARISON_SAMPLER), which samples nothing else, as the
    ///   other textures hold no depth. It uses no other resources, and nothing else that needs a
    ///   feature the engine doesn't enable, such as `double`, `half`, 64-bit integers or wave
    ///   operations.
    /// - Its structs for `V` and `P` have their fields, in order, of 32-bit scalars, vectors and
    ///   matrices: Slang lays out what a pointer points at as `#[repr(C)]` does, without padding.
    ///   glam's `Vec2`, `Vec3`, `Vec4`, `Quat`, `Mat3` and `Mat4` are `float2`, `float3`,
    ///   `float4`, `float4`, `float3x3` and `float4x4`; not `Vec3A` or `Mat3A`, whose columns
    ///   take 16 bytes to `float3`'s 12. The engine builds glam without SIMD, so each of them is
    ///   4-byte aligned, as Slang's are.
    ///
    /// The check sees each struct's size and that its fields are 32 bits each, but not what the
    /// fields are: fields of the same total size as its type's, in another order or of another
    /// kind, such as a `uint` read as a float, draw garbage without a message. It tells the push
    /// constants apart by their places and types alone, so parameters that are one `uint2`,
    /// declared where `ids` goes or swapped with it, read the IDs as their own. Nor does it see
    /// which way a matrix is laid out, or whether an index that can differ across a draw goes
    /// through `NonUniformResourceIndex`.
    ///
    /// slangc writes the SPIR-V in a directory of the compile's own in the OS's temporary
    /// directory, with the list of the files the shader read, its own and those it includes, which
    /// its [`ShaderSource`] keeps, with when the compile began, for hot reload to compile it again
    /// as they change.
    ///
    /// Fails with `renderer.bad_shader` and slangc's diagnostics if it doesn't compile, with
    /// `renderer.shader_mismatch` and each way it breaks the contract if it breaks it, with
    /// `renderer.shader_files` if slangc's directory can't be made or what it wrote there can't
    /// be read, and with `renderer.no_slangc` if slangc can't be run.
    pub fn add<V: NoUninit, P: NoUninit>(
        &mut self,
        name: &str,
        path: &Path,
        blend: Blend,
    ) -> Result<Shader<V, P>> {
        let types = Types::of::<V, P>();
        let compiled_at = SystemTime::now();
        let (spirv, files) = compile(Path::new(SLANGC), name, path)?;
        check(name, path, &spirv, &types)?;
        let index = self.push(name, spirv, None, blend, types);
        self.sources.push(ShaderSource {
            index,
            name: name.to_owned(),
            path: path.to_owned(),
            blend,
            types,
            files,
            compiled_at,
        });
        Ok(Shader::new(index, writes_depth(blend)))
    }

    /// Adds a game's shader, `name`, as [`compile_shader`] compiled it from the file at `path`,
    /// `spirv`, as `tools/cook` puts it in a game's package: as [`add`](Self::add) adds one it
    /// compiles itself, checked against the contract for its types alike, but listing no files,
    /// so that it doesn't compile again as they change.
    ///
    /// Fails with `renderer.shader_mismatch` and each way it breaks the contract if it breaks it,
    /// as SPIR-V that can't be read does.
    pub fn add_compiled<V: NoUninit, P: NoUninit>(
        &mut self,
        name: &str,
        path: &Path,
        spirv: Vec<u8>,
        blend: Blend,
    ) -> Result<Shader<V, P>> {
        let types = Types::of::<V, P>();
        check(name, path, &spirv, &types)?;
        let index = self.push(name, spirv, None, blend, types);
        self.sources.push(ShaderSource {
            index,
            name: name.to_owned(),
            path: path.to_owned(),
            blend,
            types,
            files: Vec::new(),
            compiled_at: SystemTime::now(),
        });
        Ok(Shader::new(index, writes_depth(blend)))
    }

    /// The shader [`add`](Self::add) gave for a file whose path `same` takes, drawing meshes of `V`
    /// with parameters `P` and blending as `blend` says, if one was added: its types by their names
    /// and sizes, as a snapshot's asset lines name a draw's.
    pub fn find<V: NoUninit, P: NoUninit>(
        &self,
        blend: Blend,
        same: impl Fn(&Path) -> bool,
    ) -> Option<Shader<V, P>> {
        let types = Types::of::<V, P>();
        let source = (self.sources.iter())
            .find(|source| source.blend == blend && source.types == types && same(&source.path))?;
        Some(Shader::new(source.index, writes_depth(blend)))
    }

    /// The games' shaders added so far, in the order they were added, for hot reload to compile
    /// again as their files change; the engine's own shaders aren't among them.
    pub fn sources(&self) -> &[ShaderSource] {
        &self.sources
    }

    /// Puts `compiled` in place of the shader it was compiled from, which keeps its handle: a
    /// renderer makes its pipeline anew the next time it renders, and draws with the old one
    /// until then. It takes the place of any of that shader's SPIR-V still waiting for a
    /// renderer, as it was added or replaced. A renderer that can't make its pipeline warns, and
    /// goes on drawing with the old one. A shader removed stays removed: one compiled again from
    /// its source is dropped.
    ///
    /// # Panics
    ///
    /// If `compiled` wasn't compiled from one of these shaders, as another `Shaders`' handles can
    /// stand for shaders of other types, which would draw meshes as though laid out as they are.
    pub fn replace(&mut self, compiled: Compiled) {
        let Compiled { source, spirv, .. } = compiled;
        if self.removed.contains(&source.index) {
            return;
        }
        assert!(
            self.sources.contains(&source),
            "shader {:?} was compiled from another `Shaders`' source than the one it replaces",
            source.name
        );
        let waiting = (self.changes.iter_mut()).find_map(|change| match change {
            Change::Added(added) if added.index == source.index => Some(added),
            _ => None,
        });
        if let Some(waiting) = waiting {
            waiting.spirv = spirv;
            return;
        }
        self.changes.push(Change::Added(Added {
            index: source.index,
            replacement: true,
            name: source.name,
            spirv,
            vertex_size: source.types.vertex_size,
            params_size: source.types.params_size,
            blend: source.blend,
            shadow: None,
        }));
    }

    /// Removes `shader`: its pipeline goes once the GPU is done with the frames that draw with it,
    /// every draw with it draws nothing from the frame that takes the removal on, and a game's
    /// shader leaves [`sources`](Self::sources), so that it isn't compiled again. Its handle is
    /// never given out again, so a stale one, in a world or a snapshot, never draws with another
    /// shader. Removing one removed already does nothing.
    ///
    /// # Panics
    ///
    /// If `shader` isn't one of these shaders, as another `Shaders`' handles can be past them.
    pub fn remove<V, P>(&mut self, shader: Shader<V, P>) {
        assert!(
            shader.index < self.count,
            "shader {} is one of another `Shaders`",
            shader.index
        );
        if self.removed.insert(shader.index) {
            self.sources.retain(|source| source.index != shader.index);
            self.changes.push(Change::Removed(shader.index));
        }
    }

    /// Takes `spirv`, shader `name` compiled, which draws meshes of `types`, and `shadow`, what
    /// draws its shadows if they're cut out, as [`Added::shadow`] says, each with its sampled
    /// images decorated as [`spirv::decorate_non_uniform`] says, and gives the index of its handle.
    fn push(
        &mut self,
        name: &str,
        spirv: Vec<u8>,
        shadow: Option<Vec<u8>>,
        blend: Blend,
        types: Types,
    ) -> u32 {
        let index = self.count;
        self.count += 1;
        self.changes.push(Change::Added(Added {
            index,
            replacement: false,
            name: name.to_owned(),
            spirv: spirv::decorate_non_uniform(spirv),
            vertex_size: types.vertex_size,
            params_size: types.params_size,
            blend,
            shadow: shadow.map(spirv::decorate_non_uniform),
        }));
        index
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
        let shadow = matches!(alpha, AlphaMode::Mask { .. }).then_some(model::MASKED_SHADOW_SPIRV);
        self.lazy(slot, name, spirv, shadow, blend)
    }

    /// Adds the engine's model shader for each alpha mode that none has asked for yet, as
    /// [`MeshDraw::with_material`](crate::MeshDraw::with_material) adds one the first time a
    /// material asks for it: so that a material made later, as a model's read again, adds none,
    /// whose index what's added after it would then follow.
    pub fn add_model_shaders(&mut self) {
        let modes = [
            AlphaMode::Opaque,
            AlphaMode::Mask { cutoff: 0.5 },
            AlphaMode::Blend,
        ];
        for alpha in modes {
            self.model(alpha);
        }
    }

    /// The engine's circle shader, which the build compiled, added as the model shaders are the
    /// first time it's asked for.
    pub(crate) fn circle(&mut self) -> Shader<Corner, circle::Params> {
        self.lazy(3, "circle", circle::SPIRV, None, Blend::Alpha)
    }

    /// The engine's line shader, which the build compiled, added as the model shaders are the
    /// first time it's asked for.
    pub(crate) fn line(&mut self) -> Shader<Corner, line::Params> {
        self.lazy(4, "line", line::SPIRV, None, Blend::Alpha)
    }

    /// The engine's grid shader, which the build compiled, added as the model shaders are the
    /// first time it's asked for.
    pub(crate) fn grid(&mut self) -> Shader<Vec2, grid::Params> {
        self.lazy(5, "grid", grid::SPIRV, None, Blend::Alpha)
    }

    /// The engine's text shader, which the build compiled, added as the model shaders are the
    /// first time it's asked for.
    pub(crate) fn text(&mut self) -> Shader<Corner, text::Params> {
        self.lazy(6, "text", text::SPIRV, None, Blend::Alpha)
    }

    /// The engine's shader in slot `slot` of `lazy`, `name`, compiled to `spirv`, with `shadow`
    /// to draw its shadows cut out, if they are, which draws meshes of `V` with parameters `P` and
    /// blends as `blend` says: added the first time it's asked for, and then the same.
    fn lazy<V, P>(
        &mut self,
        slot: usize,
        name: &str,
        spirv: &[u8],
        shadow: Option<&[u8]>,
        blend: Blend,
    ) -> Shader<V, P> {
        let index = match self.lazy[slot] {
            Some(index) => index,
            None => {
                let (spirv, shadow) = (spirv.to_vec(), shadow.map(<[u8]>::to_vec));
                let index = self.push(name, spirv, shadow, blend, Types::of::<V, P>());
                *self.lazy[slot].insert(index)
            }
        };
        Shader::new(index, writes_depth(blend))
    }
}

impl Default for Shaders {
    fn default() -> Self {
        let mut shaders = Self {
            changes: Vec::new(),
            count: 0,
            removed: HashSet::new(),
            lazy: [None; 7],
            sources: Vec::new(),
        };
        let types = Types::of::<Corner, Params>();
        let index = shaders.push("sprite", sprite::SPIRV.to_vec(), None, Blend::Alpha, types);
        assert_eq!(index, sprite::SHADER.index, "the sprite shader comes first");
        shaders
    }
}

/// A game's shader as [`Shaders::add`] added it, which [`ShaderSource::compile`] compiles again
/// from its file, as hot reload does when the files it reads change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderSource {
    index: u32,
    name: String,
    path: PathBuf,
    blend: Blend,
    types: Types,
    files: Vec<PathBuf>,
    compiled_at: SystemTime,
}

impl ShaderSource {
    /// The name it was added with.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The Slang file it was added from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The files it read as it was added: its own first, then those it includes, as slangc found
    /// them.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// When the compile that added it began: what its files hold from then on may not be in it.
    pub fn compiled_at(&self) -> SystemTime {
        self.compiled_at
    }

    /// Compiles the shader again from its file, and checks it against the contract for the types
    /// it was added for, as [`Shaders::add`] did, on the calling thread: slangc takes about a
    /// tenth of a second. Its [`Compiled`] lists the files the shader read this time.
    ///
    /// Fails as `Shaders::add` does.
    pub fn compile(&self) -> Result<Compiled> {
        let (spirv, files) = compile(Path::new(SLANGC), &self.name, &self.path)?;
        check(&self.name, &self.path, &spirv, &self.types)?;
        Ok(Compiled {
            source: self.clone(),
            spirv: spirv::decorate_non_uniform(spirv),
            files,
        })
    }
}

/// A game's shader compiled again, which [`Shaders::replace`] puts in place of the one it was
/// compiled from.
#[derive(Debug)]
pub struct Compiled {
    source: ShaderSource,
    /// Decorated as [`spirv::decorate_non_uniform`] says.
    spirv: Vec<u8>,
    files: Vec<PathBuf>,
}

impl Compiled {
    /// What it was compiled from.
    pub fn source(&self) -> &ShaderSource {
        &self.source
    }

    /// The files the shader read, its own first, then those it includes, as slangc found them.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
}

/// Checks `spirv`, shader `name` at `path` as slangc compiled it, against the contract that
/// [`Shaders::add`] describes for a shader that draws with `types`.
fn check(name: &str, path: &Path, spirv: &[u8], types: &Types) -> Result<()> {
    let _zone = gg_core::zone!();
    let breaches = spirv::breaches(spirv, types);
    if breaches.is_empty() {
        return Ok(());
    }
    let message = format!(
        "shader {name:?} at {} doesn't keep to the contract that `Shaders::add` describes:\n{}",
        path.display(),
        breaches.join("\n")
    );
    Err(Error::new("renderer.shader_mismatch", message))
}

/// Compiles a game's shader, `name`, from the file at `path` to SPIR-V with the slangc the build
/// found, as [`Shaders::add`] does, but unchecked, as checking it against the contract needs the
/// types it draws with: for `tools/cook`, from whose package a run adds it through
/// [`Shaders::add_compiled`], which checks it. Gives it with the files the shader read, its own
/// first, then those it includes.
///
/// Fails as `Shaders::add` does but for the contract.
pub fn compile_shader(name: &str, path: &Path) -> Result<(Vec<u8>, Vec<PathBuf>)> {
    compile(Path::new(SLANGC), name, path)
}

/// Compiles the shader `name` at `path` to SPIR-V with `slangc`, and gives it with the files the
/// shader read, its own first, then those it includes.
fn compile(slangc: &Path, name: &str, path: &Path) -> Result<(Vec<u8>, Vec<PathBuf>)> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let start = Instant::now();
    // slangc lists the files it read only beside SPIR-V it writes to a file.
    let written = Written::new().map_err(|error| {
        let temporary = std::env::temp_dir();
        Error::new(
            "renderer.shader_files",
            format!(
                "can't make a directory in {} for slangc to compile shader {name:?} at {} into",
                temporary.display(),
                path.display()
            ),
        )
        .with_source(error)
    })?;
    let mut command = Command::new(slangc);
    command
        .arg(path)
        .args(["-target", "spirv", "-fvk-use-entrypoint-name"])
        // Unoptimized: slangc's optimizer turns a struct's read through an address, as
        // `*push.frame` reads the frame's data, into reads of the fields used (2026.20's always,
        // the Vulkan SDK's 2025.23.2 where few are), which AMD's driver (25.10.30.02) compiles as
        // reads through the wrong address, losing the device. Unoptimized, slangc reads the struct
        // whole, and AMD's code reads what the shader does. Drop it once AMD's driver compiles
        // those reads right.
        .arg("-O0")
        // As glam lays matrices out. It's slangc's default too, but only for legacy reasons, as
        // Slang's docs have it: elsewhere the default is row-major.
        .arg("-matrix-layout-column-major")
        // A function marked for one stage but compiled as the other.
        .args(["-warnings-as-errors", "38006"])
        // Parameters at one binding, as the bindless set's textures are, 2D and cubes, which
        // Vulkan lets a shader declare as both, each slot read as what it holds.
        .args(["-warnings-disable", "39001"])
        .args(["-entry", "vertex_main", "-stage", "vertex"])
        .args(["-entry", "fragment_main", "-stage", "fragment"])
        .arg("-o")
        .arg(&written.spirv)
        .arg("-depfile")
        .arg(&written.depfile);
    let output = command.output().map_err(|error| {
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
    let unreadable = |file: &Path, error: io::Error| {
        Error::new(
            "renderer.shader_files",
            format!(
                "can't read {}, which slangc wrote as it compiled shader {name:?} at {}",
                file.display(),
                path.display()
            ),
        )
        .with_source(error)
    };
    let depfile =
        fs::read(&written.depfile).map_err(|error| unreadable(&written.depfile, error))?;
    let Some(files) = depfile_files(&depfile) else {
        let named = io::Error::other("it names a file whose name isn't UTF-8");
        return Err(unreadable(&written.depfile, named));
    };
    if files.is_empty() {
        let listed = io::Error::other("it lists no files");
        return Err(unreadable(&written.depfile, listed));
    }
    let spirv = fs::read(&written.spirv).map_err(|error| unreadable(&written.spirv, error))?;
    debug!(
        shader = name,
        path = %path.display(),
        ms = start.elapsed().as_millis() as u64,
        "Shader compiled"
    );
    Ok((spirv, files))
}

/// Where slangc writes a shader it compiles, and the list of the files it read: a directory of
/// the compile's own in the OS's temporary directory, removed with them as it drops. That
/// directory is shared on Unix, so its name is random and only the user can write in it: no one
/// else can have made it, or put in it anything slangc would write through or this would read.
struct Written {
    dir: PathBuf,
    spirv: PathBuf,
    depfile: PathBuf,
}

impl Written {
    /// Makes the directory, which fails if anything is at its name.
    fn new() -> io::Result<Self> {
        static COMPILES: AtomicU64 = AtomicU64::new(0);
        let compile = COMPILES.fetch_add(1, Ordering::Relaxed);
        let random = RandomState::new().hash_one(compile);
        let name = format!("gg-shader-{}-{compile}-{random:016x}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(&dir)?;
        #[cfg(not(unix))]
        fs::create_dir(&dir)?;
        Ok(Self {
            spirv: dir.join("shader.spv"),
            depfile: dir.join("shader.d"),
            dir,
        })
    }
}

impl Drop for Written {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The files that `depfile`, which slangc wrote, lists as what its output was made from, each
/// once, in order; `None` if one's name isn't a path here, as one that isn't UTF-8 isn't outside
/// Unix. It's a Makefile's rule, `target: prerequisites`, in which a backslash escapes a colon, a
/// space, a `#` or another backslash, `$$` is a `$`, and a backslash ending a line carries the
/// rule on to the next. Names are bytes, as Unix's are, which slangc writes as it was given them.
fn depfile_files(depfile: &[u8]) -> Option<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut word = Vec::new();
    let mut targets_done = false;
    let mut bytes = depfile.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        match byte {
            b'\\' => match bytes.peek() {
                Some(&next @ (b':' | b' ' | b'#' | b'\\')) => {
                    word.push(next);
                    bytes.next();
                }
                Some(b'\r' | b'\n') => {
                    // A line carried on: a space between words.
                    if bytes.next() == Some(b'\r') && bytes.peek() == Some(&b'\n') {
                        bytes.next();
                    }
                    end_word(&mut word, targets_done, &mut files)?;
                }
                _ => word.push(b'\\'),
            },
            b'$' if bytes.peek() == Some(&b'$') => {
                word.push(b'$');
                bytes.next();
            }
            // The rule's targets end at the first colon that isn't escaped.
            b':' if !targets_done => {
                targets_done = true;
                word.clear();
            }
            b'\n' => {
                end_word(&mut word, targets_done, &mut files)?;
                targets_done = false;
            }
            byte if byte.is_ascii_whitespace() => end_word(&mut word, targets_done, &mut files)?,
            byte => word.push(byte),
        }
    }
    end_word(&mut word, targets_done, &mut files)?;
    Some(files)
}

/// Ends `word`, which `files` takes once the rule's targets are done, unless it has it already.
fn end_word(word: &mut Vec<u8>, targets_done: bool, files: &mut Vec<PathBuf>) -> Option<()> {
    let word = std::mem::take(word);
    if targets_done && !word.is_empty() {
        let file = path(word)?;
        if !files.contains(&file) {
            files.push(file);
        }
    }
    Some(())
}

/// The path named by `bytes`: any bytes on Unix, and UTF-8 elsewhere, as slangc writes names on
/// Windows.
#[cfg(unix)]
fn path(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(std::ffi::OsString::from_vec(bytes).into())
}

#[cfg(not(unix))]
fn path(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(file: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/shaders")
            .join(file)
    }

    /// The names of `files`, without their directories.
    fn names(files: &[PathBuf]) -> Vec<Option<&str>> {
        (files.iter())
            .map(|file| file.file_name().and_then(|name| name.to_str()))
            .collect()
    }

    /// The shaders added or replaced, waiting to be taken, in order.
    fn adds(shaders: &Shaders) -> Vec<&Added> {
        (shaders.changes.iter())
            .filter_map(|change| match change {
                Change::Added(added) => Some(added),
                Change::Removed(_) => None,
            })
            .collect()
    }

    /// A directory of the test's own, emptied, and removed as the test ends, failed or not.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gg-shader-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("a directory");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
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

    /// Adds the shader `file`, which draws vertices `V` with parameters `P`, to `shaders`.
    fn keeps_to_it<V: NoUninit, P: NoUninit>(shaders: &mut Shaders, file: &str) {
        let added = shaders.add::<V, P>(file, &fixture(file), Blend::Opaque);
        added.unwrap_or_else(|error| panic!("{file}: {error:#}"));
    }

    /// Why the shader `file`, added to draw vertices `V` with parameters `P`, is refused, which it
    /// must be for breaking the contract, without taking a handle.
    fn refusal<V: NoUninit, P: NoUninit>(file: &str) -> String {
        let mut shaders = Shaders::default();
        let path = fixture(file);
        let error = (shaders.add::<V, P>(file, &path, Blend::Opaque)).expect_err(file);
        assert_eq!(error.code(), "renderer.shader_mismatch", "{error:#}");
        assert!(shaders.sources().is_empty(), "{file}");
        assert_eq!(shaders.changes.len(), 1, "{file}: the sprite shader alone");
        let message = error.to_string();
        assert!(message.contains(&path.display().to_string()), "{message}");
        message
    }

    #[test]
    fn the_engines_shaders_and_the_fixtures_keep_to_the_contract() {
        let models = Types::of::<ModelVertex, model::Params>();
        let engines = [
            (sprite::SPIRV, Types::of::<Corner, Params>()),
            (circle::SPIRV, Types::of::<Corner, circle::Params>()),
            (line::SPIRV, Types::of::<Corner, line::Params>()),
            (grid::SPIRV, Types::of::<Vec2, grid::Params>()),
            (text::SPIRV, Types::of::<Corner, text::Params>()),
            (model::OPAQUE_SPIRV, models),
            (model::MASKED_SPIRV, models),
            (model::BLENDED_SPIRV, models),
        ];
        for (spirv, types) in engines {
            assert!(spirv::breaches(spirv, &types).is_empty(), "{types:?}");
        }
        let mut shaders = Shaders::default();
        keeps_to_it::<[f32; 3], [f32; 4]>(&mut shaders, "flat_color.slang");
        keeps_to_it::<[f32; 5], [u32; 2]>(&mut shaders, "textured.slang");
        keeps_to_it::<[f32; 5], [u32; 3]>(&mut shaders, "level.slang");
        keeps_to_it::<[f32; 7], ()>(&mut shaders, "vertex_color.slang");
        // Given parameters it doesn't read, and reading no push constants at all.
        keeps_to_it::<[f32; 7], [f32; 4]>(&mut shaders, "vertex_color.slang");
        keeps_to_it::<[f32; 3], ()>(&mut shaders, "no_push_constants.slang");
        assert_eq!(shaders.sources().len(), 6);
    }

    #[test]
    fn a_shader_that_breaks_the_contract_says_each_way_and_takes_no_handle() {
        type Vertex = [f32; 3];
        type Color = [f32; 4];
        let says = |message: String, ways: &[&str]| {
            for way in ways {
                assert!(message.contains(way), "doesn't say {way:?}: {message}");
            }
        };
        says(
            refusal::<Color, Color>("flat_color.slang"),
            &["its vertices, Vertex, take 12 bytes, where its vertex type, [f32; 4], takes 16"],
        );
        says(
            refusal::<Vertex, Vertex>("flat_color.slang"),
            &["its parameters, Params, take 16 bytes, where its type of parameters, [f32; 3]"],
        );
        says(
            refusal::<Vertex, ()>("flat_color.slang"),
            &["it reads parameters through params, but it's added without them"],
        );
        says(
            refusal::<Vertex, Color>("push_reordered.slang"),
            &[
                "its push constants' vertices isn't a float4x4",
                "it reads clip_from_world from its push constants at byte 16",
            ],
        );
        says(
            refusal::<Vertex, Color>("push_extra.slang"),
            &[
                "its push constants' view isn't a float2",
                "its frame's data, Frame, take 260 bytes, past the 256 the engine's take",
                "it reads brightness from its push constants, past the 112 bytes",
            ],
        );
        says(
            refusal::<Vertex, Color>("float_ids.slang"),
            &["its push constants' ids isn't the address of uint2s"],
        );
        // Parameters of 8 bytes, where the IDs go, and the IDs where the parameters go, which
        // would pass for them.
        says(
            refusal::<Vertex, [u32; 2]>("ids_after_params.slang"),
            &["its push constants' params isn't the address of uint2s"],
        );
        says(
            refusal::<Vertex, Color>("writes_vertices.slang"),
            &["it writes through an address the engine pushes"],
        );
        says(
            refusal::<Vertex, Color>("other_resources.slang"),
            &[
                "it binds a buffer, tints, at set 0, binding 2",
                "it binds a storage image, canvas, at set 1, binding 0",
            ],
        );
        says(
            refusal::<[f32; 5], [u32; 2]>("wrong_bindings.slang"),
            &[
                "it binds textures of integers, textures, at set 0, binding 0",
                "it binds samplers, samplers, at set 0, binding 2",
            ],
        );
        says(
            refusal::<Vertex, Color>("vertex_inputs.slang"),
            &[
                "its vertex stage reads BaseVertex",
                "its vertex stage reads offset at location 0 as a vertex attribute",
                "DrawParameters",
            ],
        );
        says(
            refusal::<Vertex, Color>("outputs.slang"),
            &[
                "its fragment stage returns its color, color, as what isn't a float4",
                "its fragment stage writes glow at location 2",
            ],
        );
        says(
            refusal::<Vertex, Color>("no_id.slang"),
            &["its fragment stage writes no entity ID at location 1"],
        );
        says(
            refusal::<Vertex, Color>("signed_id.slang"),
            &["its fragment stage writes its ID, id, as int2: write uint2"],
        );
        says(
            refusal::<Vertex, Color>("doubles.slang"),
            &["Float64, for `double`"],
        );
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
        let textured = fixture("textured.slang");
        (shaders.add::<[f32; 5], [u32; 2]>("textured", &textured, Blend::Alpha)).expect("a shader");
        let level = fixture("level.slang");
        (shaders.add::<[f32; 5], [u32; 3]>("level", &level, Blend::Alpha)).expect("a shader");
        // The sprite shader, the three model shaders and the two games'. Each samples a texture
        // by an index an instance carries; the model shaders sample the sky's light too, by the
        // frame's indices, the same across a draw.
        assert_eq!(shaders.changes.len(), 6);
        for added in adds(&shaders) {
            let sampled = spirv::sampled_images(&added.spirv);
            assert!(
                sampled.iter().any(|&(_, decorated, _)| decorated)
                    && (sampled.iter()).all(|&(_, decorated, of)| decorated || !of),
                "{}: {sampled:?}",
                added.name
            );
        }
    }

    /// The model shaders load the frame's data whole, unoptimized as `compile` says why: reaching
    /// into it for its lights' or materials' address alone makes a pointer to a pointer in buffer
    /// memory.
    #[test]
    fn the_model_shaders_load_the_frame_whole() {
        let shaders = [
            ("opaque", model::OPAQUE_SPIRV),
            ("masked", model::MASKED_SPIRV),
            ("masked shadows", model::MASKED_SHADOW_SPIRV),
            ("blended", model::BLENDED_SPIRV),
        ];
        for (name, spirv) in shaders {
            assert_eq!(spirv::buffer_pointers_to_pointers(spirv), 0, "{name}");
        }
    }

    /// The cull shader takes every address from its push constants, as it says why, and none
    /// from buffer memory; the model shaders' frame holds some, which the check would find.
    #[test]
    fn the_cull_shader_holds_no_addresses_in_buffers() {
        assert_eq!(
            spirv::buffer_structs_holding_pointers(crate::mesh::CULL_SPIRV),
            0
        );
        let frame = spirv::buffer_structs_holding_pointers(model::OPAQUE_SPIRV);
        assert!(frame > 0, "the model shader's frame holds addresses");
    }

    #[test]
    fn a_shader_compiled_again_lists_its_files_and_replaces_itself() {
        let dir = Scratch::new("compiled-again");
        let shaders_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        for file in ["model_opaque.slang", "model.slang", "mesh.slang"] {
            fs::copy(shaders_dir.join(file), dir.0.join(file)).expect("a copy");
        }
        let path = dir.0.join("model_opaque.slang");
        let mut shaders = Shaders::default();
        let shader = (shaders.add::<ModelVertex, model::Params>("copy", &path, Blend::Opaque))
            .expect("a shader");
        let [source] = shaders.sources() else {
            panic!("one source");
        };
        assert_eq!((source.name(), source.path()), ("copy", path.as_path()));
        let read = [
            Some("model_opaque.slang"),
            Some("model.slang"),
            Some("mesh.slang"),
        ];
        assert_eq!(names(source.files()), read, "as it was added");

        // A change to the file it includes.
        let include = dir.0.join("model.slang");
        let text = fs::read_to_string(&include).expect("the include");
        let dimmer = text.replace("* input.color", "* input.color * 0.5");
        fs::write(&include, dimmer).expect("the include changed");
        let compiled = source.compile().expect("the shader compiles again");
        assert_eq!(names(compiled.files()), read);
        let exist = compiled.files().iter().all(|file| file.exists());
        assert!(exist, "{compiled:?}");
        assert_eq!(compiled.source(), source);

        // Before a renderer takes the first, it takes the first's place; after, it's queued, and
        // the next takes its place in turn.
        let first_spirv = adds(&shaders)[1].spirv.clone();
        let source = source.clone();
        shaders.replace(compiled);
        assert_eq!(shaders.changes.len(), 2, "the sprite shader and the copy");
        assert_eq!(adds(&shaders)[1].index, shader.index);
        assert_ne!(adds(&shaders)[1].spirv, first_spirv);
        assert!(
            !adds(&shaders)[1].replacement,
            "still the shader added, which comes next"
        );
        shaders.changes.clear();
        shaders.replace(source.compile().expect("the shader compiles again"));
        shaders.replace(source.compile().expect("the shader compiles again"));
        assert_eq!(shaders.changes.len(), 1);
        assert_eq!(adds(&shaders)[0].index, shader.index);
        assert_eq!(adds(&shaders)[0].blend, Blend::Opaque);
        assert!(
            adds(&shaders)[0].replacement,
            "in place of the one a renderer has"
        );

        // Removed, it leaves the sources, so that it isn't compiled again, and one compiled from
        // its source already is dropped; removing it again does nothing.
        shaders.changes.clear();
        let compiled = source.compile().expect("the shader compiles again");
        shaders.remove(shader);
        shaders.remove(shader);
        assert!(shaders.sources().is_empty());
        assert!(matches!(shaders.changes[..], [Change::Removed(1)]));
        shaders.replace(compiled);
        assert_eq!(shaders.changes.len(), 1, "the replacement was dropped");
        let found = shaders.find::<ModelVertex, model::Params>(Blend::Opaque, |_| true);
        assert!(found.is_none(), "a removed shader isn't found");

        // A change that doesn't compile, and one that breaks the contract.
        fs::write(&include, "struct Vertex {").expect("the include broken");
        let error = source.compile().expect_err("the include doesn't compile");
        assert_eq!(error.code(), "renderer.bad_shader");
        // The vertex, the first struct with texture coordinates, a float wider than its type.
        let wider = text.replacen("float2 uv;", "float2 uv;\n    float extra;", 1);
        assert_ne!(wider, text, "the vertex is wider");
        fs::write(&include, wider).expect("the include changed");
        let error = source.compile().expect_err("a vertex too wide");
        assert_eq!(error.code(), "renderer.shader_mismatch");
    }

    #[test]
    #[should_panic(expected = "compiled from another `Shaders`' source")]
    fn a_shader_compiled_from_another_shaders_source_replaces_nothing() {
        let mut other = Shaders::default();
        let colored = fixture("vertex_color.slang");
        (other.add::<[f32; 7], ()>("colored", &colored, Blend::Opaque)).expect("a shader");
        let compiled = other.sources()[0].compile().expect("a shader");
        let mut shaders = Shaders::default();
        let flat = fixture("flat_color.slang");
        (shaders.add::<[f32; 3], [f32; 4]>("flat", &flat, Blend::Opaque)).expect("a shader");
        shaders.replace(compiled);
    }

    #[test]
    fn depfiles_list_the_files_their_rules_name() {
        let files = |depfile: &[u8]| depfile_files(depfile).expect("names that are paths");
        // As slangc writes them on Windows: a drive's colon and every backslash escaped.
        let windows = br"C\:\\Temp\\out.spv: C\:\\dev\\a\ dir\\main.slang C\:\\dev\\shared.slang";
        assert_eq!(
            files(windows),
            [r"C:\dev\a dir\main.slang", r"C:\dev\shared.slang"].map(PathBuf::from)
        );
        let unix = b"/tmp/out.spv: /src/main.slang \\\n  /src/a\\#b.slang /src/cost$$.slang \\\r\n  /src/main.slang\n";
        assert_eq!(
            files(unix),
            ["/src/main.slang", "/src/a#b.slang", "/src/cost$.slang"].map(PathBuf::from)
        );
        // A rule per line, and a line without one.
        let rules = b"a.spv: one.slang\nb.spv: two.slang\nstray\n";
        assert_eq!(files(rules), ["one.slang", "two.slang"].map(PathBuf::from));
        assert!(files(b"").is_empty());
        // Names in UTF-8, as slangc writes them on Windows; and one that isn't, which on Unix is
        // a name like any other.
        let japanese = files("out.spv: 日本.slang".as_bytes());
        assert_eq!(japanese, [PathBuf::from("日本.slang")]);
        let latin1 = b"out.spv: caf\xe9.slang\n";
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let file = Path::new(std::ffi::OsStr::from_bytes(b"caf\xe9.slang"));
            assert_eq!(files(latin1), [file.to_owned()]);
        }
        #[cfg(not(unix))]
        assert_eq!(depfile_files(latin1), None);
    }

    #[test]
    fn a_missing_slangc_is_its_own_error() {
        let flat = fixture("flat_color.slang");
        let error = compile(Path::new("no-such-slangc"), "flat", &flat).expect_err("no slangc");
        assert_eq!(error.code(), "renderer.no_slangc");
    }
}
