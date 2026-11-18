//! Shaders: Slang compiled to SPIR-V by slangc, games' as they're added and the engine's as it
//! builds. Each draws meshes of one vertex type in the game's view.

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

use crate::model::{self, AlphaMode};
use crate::packed::PackedVertex;
use crate::sprite::{self, Corner, Params};
use crate::{circle, grid, line, text};

/// The slangc the build found and compiled the renderer's shaders with.
const SLANGC: &str = env!("GG_SLANGC");

/// A shader from [`Shaders::add`], drawing meshes of vertex type `V` with per-draw parameters `P`.
pub struct Shader<V, P = ()> {
    pub(crate) index: u32,
    /// Whether it writes depth ([`writes_depth`]).
    pub(crate) opaque: bool,
    types: PhantomData<fn() -> (V, P)>,
}

// By hand, so `V` and `P` needn't be `Clone` or `Debug`.
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

/// `spirv`, an engine shader the build compiled, its sampled images decorated per
/// [`spirv::decorate_non_uniform`], for a pipeline made apart from [`Shaders`].
pub(crate) fn non_uniform(spirv: &[u8]) -> Vec<u8> {
    spirv::decorate_non_uniform(spirv.to_vec())
}

/// Whether a shader blending as `blend` writes depth as well as testing it. An opaque one does, so
/// what's behind it, drawn later, doesn't show. One that blends doesn't, so what's behind shows
/// through it, as through its transparent margins.
pub(crate) fn writes_depth(blend: Blend) -> bool {
    blend == Blend::Opaque
}

impl<V, P> fmt::Debug for Shader<V, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Shader").field(&self.index).finish()
    }
}

/// The shaders a renderer draws meshes with, the engine's sprite shader first.
///
/// Each is compiled as it's added, and its SPIR-V kept until
/// [`Uploads::take`](crate::Uploads::take) takes it, for a renderer to make its pipeline when it
/// next renders. A game's shader compiled again replaces it the same way; removals go in order with
/// them. A renderer draws with one `Shaders`' shaders only.
pub struct Shaders {
    /// Added, replaced or removed since last taken, in order.
    pub(crate) changes: Vec<Change>,
    /// How many were ever added.
    count: u32,
    /// Never handed out again.
    removed: HashSet<u32>,
    /// Handles of the engine's shaders added when first asked for, by [`LAZY`]'s slots.
    lazy: [Option<u32>; LAZY.len()],
    /// Games' shaders in order added, less those removed.
    sources: Vec<ShaderSource>,
}

/// The names of the engine's shaders added when first asked for, by slot: the model shaders, then
/// circle, line, grid and text.
const LAZY: [&str; 7] = [
    "model",
    "model masked",
    "model blended",
    "circle",
    "line",
    "grid",
    "text",
];

/// A change to a renderer's shaders, carried by [`Uploads::take`](crate::Uploads::take).
pub(crate) enum Change {
    /// Added, or compiled again in place of one.
    Added(Added),
    /// By index.
    Removed(u32),
}

pub(crate) struct Added {
    pub(crate) index: u32,
    /// Compiled again in place of one a renderer has, rather than added after them.
    pub(crate) replacement: bool,
    pub(crate) name: String,
    pub(crate) spirv: Vec<u8>,
    /// In bytes.
    pub(crate) vertex_size: usize,
    /// In bytes.
    pub(crate) params_size: usize,
    pub(crate) blend: Blend,
    /// Whether it writes IDs, as all but the engine's grid do: that's for seeing by, and leaves the
    /// IDs under it.
    pub(crate) ids: bool,
    /// What draws its shadows when it writes depth and they're cut out as it is: a module of its
    /// vertex shader and `shadow_main`, a fragment shader discarding where it does, as the engine's
    /// masked model shader has. Other depth-writing shaders' shadows are whole triangles, drawn by
    /// their vertex shaders alone.
    pub(crate) shadow: Option<Vec<u8>>,
    /// What draws the meshlets a view keeps of a mesh: for the engine's opaque and masked model
    /// shaders, which views draw meshlet meshes with.
    pub(crate) meshlets: Option<Box<Meshlets>>,
}

/// A model shader's meshlet modules, each of the entry points named:
///
/// - `vertices`: `meshlet_vertex_main` and `fragment_main`, through vertex shaders.
/// - `vertices_shadow`: those and `shadow_main`, for shadows cut out ([`Added::shadow`]).
/// - `mesh`: `mesh_main` alone. Its pipelines take the shader's own fragment shaders, as
///   [`GraphicsDesc::mesh`](gg_gpu::GraphicsDesc::mesh) says.
/// - `visibility`: `meshlet_visibility_main` and `visibility_main`, into the visibility buffer
///   through vertex shaders.
/// - `visibility_mesh`, `visibility_primitive`: `mesh_visibility_main` alone and
///   `visibility_primitive_main` alone, into it through a mesh shader.
/// - `normals`: `vertex_main` and `normal_main`, by indices, for the view's reflections where it
///   draws other than into the visibility buffer. Mesh shader pipelines take its fragment shader
///   too.
/// - `vertices_normals`: `meshlet_vertex_main` and `normal_main`, the same through vertex shaders.
pub(crate) struct Meshlets {
    pub(crate) vertices: Vec<u8>,
    pub(crate) vertices_shadow: Option<Vec<u8>>,
    pub(crate) mesh: Vec<u8>,
    pub(crate) visibility: Vec<u8>,
    pub(crate) visibility_mesh: Vec<u8>,
    pub(crate) visibility_primitive: Vec<u8>,
    pub(crate) normals: Vec<u8>,
    pub(crate) vertices_normals: Vec<u8>,
}

/// The sizes and names of a shader's vertex and parameter types, which the contract's check
/// compares its structs with.
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
    /// How many it has handed out, the sprite shader and the engine's added ones among them: the
    /// next one's index.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Compiles the Slang shader at `path`, drawing meshes of vertex type `V` with per-draw
    /// parameters `P`, such as a color (`()` for none). `name` names it in errors and GPU captures.
    /// `blend` ([`Blend`]) says how its color goes over what's drawn before it.
    ///
    /// Every shader tests the view's depth, drawing where it's as near as what's there or nearer. A
    /// [`Blend::Opaque`] one writes depth too; one that blends doesn't, so what's behind shows
    /// through. What an opaque one draws casts the shadow-casting light's shadows, of whole
    /// triangles as its vertex shader places them. For those it's pushed the shadow map's
    /// world-to-clip matrix and size in place of the view's, and frame data without the sky,
    /// shadows, clusters, ambient occlusion or fog.
    ///
    /// Vulkan checks nothing about a shader, so the engine checks the SPIR-V slangc writes against
    /// this contract:
    ///
    /// - Entry points: `vertex_main`, a vertex shader, and `fragment_main`, a fragment shader
    ///   returning a struct of two or three outputs and nothing else:
    ///   - the color, `float4 : SV_Target0`, linear RGBA, which may pass 1 (the view is HDR);
    ///   - the ID, `uint2 : SV_Target1`, kept where the draw shows for [picks](crate::Pick):
    ///     `ids[instance]`, read by the vertex shader and passed on `nointerpolation`, or 0 for no
    ///     entity;
    ///   - optionally the motion, `float4 : SV_Target2`: how far the point moved in the world since
    ///     the last frame, in XYZ, 0 in W. That's `mul(transforms[instance], float4(position,
    ///     1.0))` less `mul(transforms[instance + previous], float4(position, 1.0))`, passed on
    ///     interpolated by the vertex shader.
    ///
    ///   TAA ([`Antialiasing::Taa`](crate::Antialiasing::Taa)) takes each pixel's history by the
    ///   motion of what's opaque there. A depth-writing shader returning none gets zeros returned
    ///   for it, so what it draws moves with the camera alone. A blending shader's motion goes
    ///   unused, as what's behind keeps its own. A blending shader should discard what's fully
    ///   transparent, after sampling, as the engine's do: it changes no pixel, but its ID would
    ///   hide what's behind from picks.
    /// - Each run of consecutive draws of one mesh with it is one instanced draw, an instance per
    ///   draw. Its push constants are `struct { float4x4 clip_from_world; V* vertices; float4x4*
    ///   transforms; uint2* ids; P* params; float2 view; Frame* frame; uint2* meshlets; uint draw;
    ///   uint previous; }`, 128 bytes:
    ///   - `clip_from_world`: the camera's view and projection, from the world to Vulkan's clip
    ///     space, jittered a fraction of a pixel each frame by antialiasing.
    ///   - `vertices`: the mesh's vertices.
    ///   - `transforms`: each instance's, from the mesh's space to the world's, for
    ///     `mul(clip_from_world, mul(transforms[instance], float4(position, 1.0)))`.
    ///   - `ids`: each instance's, the low and high halves of a 64-bit number.
    ///   - `params`: one `P` per instance, back to back.
    ///   - `view`: the view's size in pixels, across then down, for what's sized on screen.
    ///   - `frame`: the frame's data, below.
    ///   - `meshlets`, `draw`: the engine's model shaders' own, 0 for any other.
    ///   - `previous`: how many transforms past each instance's its last frame's is, for the
    ///     motion. Set for the world's opaque meshes in the game's view; 0 for the rest, which the
    ///     engine knows none of (a layer's draws, made each frame), so they read their own and
    ///     stand still.
    ///
    ///   A shader without parameters leaves out `params`, and so the fields after it. A shader may
    ///   end the struct after the last field it reads; one reading none has none.
    ///
    ///   The frame's data is `struct Frame { float3 camera; float time; float exposure; uint
    ///   light_count; Light* lights; Material* materials; float4 eye; uint sky; uint irradiance;
    ///   uint specular; uint specular_levels; uint brdf; float sky_luminance; Shadows shadows;
    ///   Clusters clusters; uint ambient_occlusion; uint show; Fog fog; Aerial aerial; Reflected
    ///   reflections; }`, or a struct of as many of its first fields: the engine adds fields at its
    ///   end. `shaders/mesh.slang` declares it and its structs, and says what each field holds and
    ///   when it's 0. Beyond that:
    ///   - `lights` are [`RenderData`](crate::RenderData)'s: those lighting everywhere (directional
    ///     lights and those without a range) first, then the rest, each in given order.
    ///   - `ambient_occlusion` is the view's [`AmbientOcclusion`](crate::AmbientOcclusion). Opaque
    ///     fragments load it at `SV_Position`'s XY and darken the sky's light by it. It's 0 in
    ///     every pass but the view's own, the canvas's among them.
    ///   - `fog`: the view fogs what opaque shaders draw, in its own pass after them. A blending
    ///     shader fogs itself if it likes, as `shaders/fog.slang` does, or draws unfogged.
    ///   - `aerial`'s `luminance` is 0 in the canvas's pass too.
    ///   - `reflections` are the view's [`Reflections`](crate::Reflections), which opaque fragments
    ///     sample at their pixel in place of the sky's specular light.
    /// - It reads only the vertex at `SV_VulkanVertexID` and the instance at `SV_VulkanInstanceID`:
    ///   no vertex attributes, and not `SV_VertexID` or `SV_InstanceID`, which need a feature the
    ///   engine doesn't enable. A fragment shader reading its instance's parameters gets the
    ///   instance from the vertex shader, `nointerpolation`. It writes nothing through the
    ///   addresses, which needs features not enabled either.
    /// - It may sample:
    ///   - the bindless set's `[[vk::binding(0, 0)]] Texture2D textures[]` by a
    ///     [`Texture`](crate::Texture) in its parameters, a `uint`, with `[[vk::binding(1, 0)]]
    ///     SamplerState samplers[]` by a [`Sampler`](crate::Sampler)'s index, a `uint` too, each
    ///     through `NonUniformResourceIndex`, as one draw's instances can differ;
    ///   - the cubes the frame's data names, through `[[vk::binding(0, 0)]] TextureCube cubes[]`,
    ///     the same binding;
    ///   - the shadow maps the frame's data names, by comparison alone (`SampleCmp`), through
    ///     `[[vk::binding(1, 0)]] SamplerComparisonState comparisons[]`, the samplers' binding, at
    ///     [`COMPARISON_SAMPLER`](gg_gpu::COMPARISON_SAMPLER). That samples nothing else, as other
    ///     textures hold no depth.
    ///
    ///   It uses no other resources, and nothing else needing a feature the engine doesn't enable,
    ///   such as `double`, `half`, 64-bit integers or wave operations.
    /// - Its `V` and `P` structs have their fields in order, of 32-bit scalars, vectors and
    ///   matrices: Slang lays out what a pointer points at as `#[repr(C)]` does, without padding.
    ///   `P` is whole 4-byte words even for a shader reading no parameters, as each draw's
    ///   parameters follow the last's, which other shaders read by words. glam's `Vec2`, `Vec3`,
    ///   `Vec4`, `Quat`, `Mat3` and `Mat4` are `float2`, `float3`, `float4`, `float4`, `float3x3`
    ///   and `float4x4`; not `Vec3A` or `Mat3A`, whose columns take 16 bytes to `float3`'s 12. The
    ///   engine builds glam without SIMD, so each is 4-byte aligned, as Slang's are.
    ///
    /// The check sees each struct's size and that its fields are 32 bits, not what they are: fields
    /// of the right total size in another order or kind, such as a `uint` read as a float, draw
    /// garbage silently. It tells push constants apart by place and type alone, so parameters of
    /// one `uint2`, declared where `ids` goes or swapped with it, read the IDs as their own. Nor
    /// does it see a matrix's layout, or whether an index that can differ across a draw goes
    /// through `NonUniformResourceIndex`.
    ///
    /// slangc writes the SPIR-V, and the list of files the shader read (its own and its includes),
    /// to a directory of its own in the OS's temporary directory. [`ShaderSource`] keeps the list
    /// and the compile's start time, for hot reload.
    ///
    /// # Errors
    ///
    /// - `renderer.bad_shader`, with slangc's diagnostics: it doesn't compile.
    /// - `renderer.shader_mismatch`, with each breach: it breaks the contract.
    /// - `renderer.shader_files`: slangc's directory can't be made, or what it wrote can't be read.
    /// - `renderer.no_slangc`: slangc can't be run.
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
        let index = self.push(name, spirv, None, None, blend, true, types);
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

    /// Adds a game's shader `name` from `spirv`, which [`compile_shader`] compiled from `path` into
    /// a package (`tools/cook`). It's checked against the contract as [`add`](Self::add) checks,
    /// but lists no files, so it isn't compiled again as they change.
    ///
    /// # Errors
    ///
    /// - `renderer.shader_mismatch`, with each breach: it breaks the contract, or its SPIR-V can't
    ///   be read.
    pub fn add_compiled<V: NoUninit, P: NoUninit>(
        &mut self,
        name: &str,
        path: &Path,
        spirv: Vec<u8>,
        blend: Blend,
    ) -> Result<Shader<V, P>> {
        let types = Types::of::<V, P>();
        check(name, path, &spirv, &types)?;
        let index = self.push(name, spirv, None, None, blend, true, types);
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

    /// The shader [`add`](Self::add) gave for a file whose path `same` accepts, drawing `V` with
    /// `P` and blending as `blend` says, if any. Types match by name and size, as a snapshot's
    /// asset lines name a draw's.
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

    /// Games' shaders in order added, for hot reload to compile again as their files change. The
    /// engine's aren't among them.
    pub fn sources(&self) -> &[ShaderSource] {
        &self.sources
    }

    /// Puts `compiled` in place of its source shader, keeping its handle. A renderer makes the
    /// pipeline anew when it next renders, drawing with the old one until then, or warns and keeps
    /// the old one if it can't. It replaces any of that shader's SPIR-V still waiting for a
    /// renderer. A removed shader stays removed: one compiled again from its source is dropped.
    ///
    /// # Panics
    ///
    /// - `compiled` wasn't compiled from one of these shaders. Another `Shaders`' handles can stand
    ///   for shaders of other types, which would read meshes as laid out as theirs.
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
            ids: true,
            shadow: None,
            meshlets: None,
        }));
    }

    /// Removes `shader`. Its pipeline goes once the GPU is done with the frames using it, and its
    /// draws draw nothing from the frame that takes the removal on. A game's shader leaves
    /// [`sources`](Self::sources), so it isn't compiled again. Its handle is never given out again,
    /// so a stale one, in a world or a snapshot, never draws with another shader. Removing it again
    /// does nothing.
    ///
    /// # Panics
    ///
    /// - `shader` isn't one of these: another `Shaders`' handles can be past them.
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

    /// Takes shader `name`'s `spirv`, drawing meshes of `types`, made [`prepared`]; `shadow`
    /// ([`Added::shadow`]) and `meshlets`, their sampled images decorated per
    /// [`spirv::decorate_non_uniform`]; `blend`; and `ids` ([`Added::ids`]). Gives its handle's
    /// index.
    #[expect(
        clippy::too_many_arguments,
        reason = "each says what one of the shaders is"
    )]
    fn push(
        &mut self,
        name: &str,
        spirv: Vec<u8>,
        shadow: Option<Vec<u8>>,
        meshlets: Option<&model::MeshletSpirv>,
        blend: Blend,
        ids: bool,
        types: Types,
    ) -> u32 {
        let index = self.count;
        self.count += 1;
        self.changes.push(Change::Added(Added {
            index,
            replacement: false,
            name: name.to_owned(),
            spirv: prepared(spirv),
            vertex_size: types.vertex_size,
            params_size: types.params_size,
            blend,
            ids,
            shadow: shadow.map(spirv::decorate_non_uniform),
            meshlets: meshlets.map(|meshlets| {
                let module = |spirv: &[u8]| spirv::decorate_non_uniform(spirv.to_vec());
                let shadow = meshlets.vertices_shadow;
                Box::new(Meshlets {
                    vertices: module(meshlets.vertices),
                    // Empty for a shadow that isn't cut out.
                    vertices_shadow: (!shadow.is_empty()).then(|| module(shadow)),
                    mesh: module(meshlets.mesh),
                    visibility: module(meshlets.visibility),
                    visibility_mesh: module(meshlets.visibility_mesh),
                    visibility_primitive: module(meshlets.visibility_primitive),
                    normals: module(meshlets.normals),
                    vertices_normals: module(meshlets.vertices_normals),
                })
            }),
        }));
        index
    }

    /// The engine's model shader for materials covering what's behind as `alpha` says. It's added
    /// like a game's when first asked for: a renderer without models makes no pipeline for it, and
    /// games' handles come where they would have.
    pub(crate) fn model(&mut self, alpha: AlphaMode) -> Shader<PackedVertex, model::Params> {
        let (slot, spirv, blend) = match alpha {
            AlphaMode::Opaque => (0, model::OPAQUE_SPIRV, Blend::Opaque),
            AlphaMode::Mask { .. } => (1, model::MASKED_SPIRV, Blend::Opaque),
            AlphaMode::Blend => (2, model::BLENDED_SPIRV, Blend::Alpha),
        };
        let shadow = matches!(alpha, AlphaMode::Mask { .. }).then_some(model::MASKED_SHADOW_SPIRV);
        // Blended draws go in order, by indices.
        let meshlets = match alpha {
            AlphaMode::Opaque => Some(model::MeshletSpirv {
                vertices: model::OPAQUE_MESHLETS_SPIRV,
                vertices_shadow: &[],
                mesh: model::OPAQUE_MESH_SPIRV,
                visibility: model::OPAQUE_VISIBILITY_SPIRV,
                visibility_mesh: model::OPAQUE_VISIBILITY_MESH_SPIRV,
                visibility_primitive: model::OPAQUE_VISIBILITY_PRIMITIVE_SPIRV,
                normals: model::OPAQUE_NORMALS_SPIRV,
                vertices_normals: model::OPAQUE_MESHLETS_NORMALS_SPIRV,
            }),
            AlphaMode::Mask { .. } => Some(model::MeshletSpirv {
                vertices: model::MASKED_MESHLETS_SPIRV,
                vertices_shadow: model::MASKED_MESHLETS_SHADOW_SPIRV,
                mesh: model::MASKED_MESH_SPIRV,
                visibility: model::MASKED_VISIBILITY_SPIRV,
                visibility_mesh: model::MASKED_VISIBILITY_MESH_SPIRV,
                visibility_primitive: model::MASKED_VISIBILITY_PRIMITIVE_SPIRV,
                normals: model::MASKED_NORMALS_SPIRV,
                vertices_normals: model::MASKED_MESHLETS_NORMALS_SPIRV,
            }),
            AlphaMode::Blend => None,
        };
        self.lazy(slot, spirv, shadow, meshlets.as_ref(), blend, true)
    }

    /// Adds the engine's model shader for each alpha mode not yet asked for, as
    /// [`MeshDraw::with_material`](crate::MeshDraw::with_material) does on first use. Then a
    /// material made later, as by a model read again, adds none, which would shift the indices of
    /// what's added after it.
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

    /// Added when first asked for, as the model shaders are.
    pub(crate) fn circle(&mut self) -> Shader<Corner, circle::Params> {
        self.lazy(3, circle::SPIRV, None, None, Blend::Alpha, true)
    }

    /// Added when first asked for, as the model shaders are.
    pub(crate) fn line(&mut self) -> Shader<Corner, line::Params> {
        self.lazy(4, line::SPIRV, None, None, Blend::Alpha, true)
    }

    /// Added when first asked for, as the model shaders are.
    pub(crate) fn grid(&mut self) -> Shader<Vec2, grid::Params> {
        self.lazy(5, grid::SPIRV, None, None, Blend::Alpha, false)
    }

    /// Added when first asked for, as the model shaders are.
    pub(crate) fn text(&mut self) -> Shader<Corner, text::Params> {
        self.lazy(6, text::SPIRV, None, None, Blend::Alpha, true)
    }

    /// The engine's shaders added so far when first asked for, each with its name and index, in
    /// the order added. Each added later takes the next index, shifting those of games' shaders
    /// added after it: a restored run adds them again in place ([`add_engine`](Self::add_engine)).
    pub fn engine(&self) -> impl Iterator<Item = (&'static str, u32)> + use<> {
        let mut added: [_; LAZY.len()] =
            std::array::from_fn(|slot| self.lazy[slot].map(|index| (index, LAZY[slot])));
        added.sort_unstable();
        let added = added.into_iter().flatten();
        added.map(|(index, name)| (name, index))
    }

    /// Adds the engine's shader `name` (as [`engine`](Self::engine) names it) as a draw first
    /// asking for it would, unless it's added already. Gives its index; `None` for no such name.
    pub fn add_engine(&mut self, name: &str) -> Option<u32> {
        let shader = match LAZY.iter().position(|&lazy| lazy == name)? {
            0 => self.model(AlphaMode::Opaque).index,
            1 => self.model(AlphaMode::Mask { cutoff: 0.5 }).index,
            2 => self.model(AlphaMode::Blend).index,
            3 => self.circle().index,
            4 => self.line().index,
            5 => self.grid().index,
            _ => self.text().index,
        };
        Some(shader)
    }

    /// The engine's shader in `lazy`'s `slot`, added when first asked for, then the same. `shadow`
    /// draws its shadows cut out and `meshlets` its meshlets, if it has them; `ids` is as
    /// [`Added::ids`] says.
    fn lazy<V, P>(
        &mut self,
        slot: usize,
        spirv: &[u8],
        shadow: Option<&[u8]>,
        meshlets: Option<&model::MeshletSpirv>,
        blend: Blend,
        ids: bool,
    ) -> Shader<V, P> {
        let index = match self.lazy[slot] {
            Some(index) => index,
            None => {
                let (spirv, shadow) = (spirv.to_vec(), shadow.map(<[u8]>::to_vec));
                let types = Types::of::<V, P>();
                let index = self.push(LAZY[slot], spirv, shadow, meshlets, blend, ids, types);
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
            lazy: [None; LAZY.len()],
            sources: Vec::new(),
        };
        let types = Types::of::<Corner, Params>();
        let index = shaders.push(
            "sprite",
            sprite::SPIRV.to_vec(),
            None,
            None,
            Blend::Alpha,
            true,
            types,
        );
        assert_eq!(index, sprite::SHADER.index, "the sprite shader comes first");
        shaders
    }
}

/// A game's shader as [`Shaders::add`] added it. [`ShaderSource::compile`] compiles it again, as
/// hot reload does when its files change.
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

    /// The files it read as added: its own first, then its includes, as slangc found them.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// When the compile adding it began: its files may have changed since.
    pub fn compiled_at(&self) -> SystemTime {
        self.compiled_at
    }

    /// Compiles it again and checks it against the contract for its types, as [`Shaders::add`] did,
    /// on the calling thread: slangc takes about a tenth of a second. Its [`Compiled`] lists the
    /// files read this time.
    ///
    /// Fails as `Shaders::add` does.
    pub fn compile(&self) -> Result<Compiled> {
        let (spirv, files) = compile(Path::new(SLANGC), &self.name, &self.path)?;
        check(&self.name, &self.path, &spirv, &self.types)?;
        Ok(Compiled {
            source: self.clone(),
            spirv: prepared(spirv),
            files,
        })
    }
}

/// `spirv` as a renderer makes pipelines from it: its sampled images decorated per
/// [`spirv::decorate_non_uniform`], and a fragment shader returning no motion made to return zeros,
/// per [`spirv::return_no_motion`].
fn prepared(spirv: Vec<u8>) -> Vec<u8> {
    spirv::return_no_motion(spirv::decorate_non_uniform(spirv))
}

/// A game's shader compiled again, for [`Shaders::replace`].
#[derive(Debug)]
pub struct Compiled {
    source: ShaderSource,
    /// As [`prepared`] makes it.
    spirv: Vec<u8>,
    files: Vec<PathBuf>,
}

impl Compiled {
    pub fn source(&self) -> &ShaderSource {
        &self.source
    }

    /// The files read this time, ordered as in [`ShaderSource::files`].
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
}

/// Checks `spirv`, shader `name` at `path`, against [`Shaders::add`]'s contract for `types`.
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

/// Compiles a game's shader `name` at `path` to SPIR-V with the build's slangc, unchecked.
///
/// It acts as [`Shaders::add`] does, but the check needs its types. For `tools/cook`: a run adds
/// the package's SPIR-V through [`Shaders::add_compiled`], which checks it. Gives the files read
/// too, its own first, then its includes.
///
/// Fails as `Shaders::add` does, but for the contract.
pub fn compile_shader(name: &str, path: &Path) -> Result<(Vec<u8>, Vec<PathBuf>)> {
    compile(Path::new(SLANGC), name, path)
}

/// Compiles shader `name` at `path` with `slangc`, giving the files read too, its own first.
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
        // Unoptimized: slangc's optimizer turns a struct read through an address, as `*push.frame`
        // is, into reads of the fields used (2026.20 always, the Vulkan SDK's 2025.23.2 where few
        // are). AMD's driver (25.10.30.02) compiles those as reads through the wrong address,
        // losing the device. Unoptimized, slangc reads the struct whole, which AMD compiles right.
        // Drop it once AMD's driver does.
        .arg("-O0")
        // As glam lays matrices out. slangc's default too, but only for legacy reasons, per Slang's
        // docs: elsewhere the default is row-major.
        .arg("-matrix-layout-column-major")
        // A function marked for one stage but compiled as the other.
        .args(["-warnings-as-errors", "38006"])
        // Parameters sharing a binding, as the bindless set's 2D textures and cubes do. Vulkan
        // allows it, each slot read as what it holds.
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
    // One per line, without the carriage returns slangc adds on Windows.
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

/// Where slangc writes a compile's SPIR-V and file list: a directory of its own in the OS's
/// temporary directory, removed on drop. That's shared on Unix, so the name is random and only the
/// user can write in it: no one else can have made it, or planted anything slangc would write
/// through or this would read.
struct Written {
    dir: PathBuf,
    spirv: PathBuf,
    depfile: PathBuf,
}

impl Written {
    /// Fails if anything is at its name.
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

/// The files `depfile` (slangc's) lists as its output's sources, each once, in order. `None` if a
/// name isn't a path here, as non-UTF-8 isn't outside Unix. It's a Makefile rule, `target:
/// prerequisites`: a backslash escapes a colon, a space, a `#` or a backslash, `$$` is `$`, and a
/// backslash ending a line continues the rule. Names are bytes, as on Unix, which slangc writes as
/// given.
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
                    // A continued line: a space between words.
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

/// Ends `word`, adding it to `files` once the targets are done, unless already there.
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

/// `bytes` as a path: any bytes on Unix, UTF-8 elsewhere, as slangc writes names on Windows.
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

    /// File names, without directories.
    fn names(files: &[PathBuf]) -> Vec<Option<&str>> {
        (files.iter())
            .map(|file| file.file_name().and_then(|name| name.to_str()))
            .collect()
    }

    /// Added or replaced, waiting to be taken, in order.
    fn adds(shaders: &Shaders) -> Vec<&Added> {
        (shaders.changes.iter())
            .filter_map(|change| match change {
                Change::Added(added) => Some(added),
                Change::Removed(_) => None,
            })
            .collect()
    }

    /// The test's own directory, emptied, and removed as the test ends, failed or not.
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

    /// Adds shader `file`, drawing `V` with `P`, to `shaders`.
    fn keeps_to_it<V: NoUninit, P: NoUninit>(shaders: &mut Shaders, file: &str) {
        let added = shaders.add::<V, P>(file, &fixture(file), Blend::Opaque);
        added.unwrap_or_else(|error| panic!("{file}: {error:#}"));
    }

    /// Why shader `file`, drawing `V` with `P`, is refused for breaking the contract, taking no
    /// handle.
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
        let models = Types::of::<PackedVertex, model::Params>();
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
        keeps_to_it::<[f32; 3], [f32; 4]>(&mut shaders, "moving.slang");
        assert_eq!(shaders.sources().len(), 7);
    }

    /// A shader returning no motion returns zeros there, whether it writes depth or blends; one
    /// returning its own keeps it. Each keeps to the contract.
    #[test]
    fn shaders_return_motion() {
        let mut shaders = Shaders::default();
        let (flat, moving) = (fixture("flat_color.slang"), fixture("moving.slang"));
        let added = [
            ("opaque", &flat, Blend::Opaque),
            ("blended", &flat, Blend::Alpha),
            ("moving", &moving, Blend::Opaque),
        ];
        for (name, path, blend) in added {
            (shaders.add::<[f32; 3], [f32; 4]>(name, path, blend)).expect("a shader");
        }
        let types = Types::of::<[f32; 3], [f32; 4]>();
        let locations: Vec<(&str, Vec<u32>)> = (adds(&shaders).into_iter().skip(1))
            .map(|added| {
                let breaches = spirv::breaches(&added.spirv, &types);
                assert!(breaches.is_empty(), "{}: {breaches:?}", added.name);
                (added.name.as_str(), spirv::fragment_locations(&added.spirv))
            })
            .collect();
        assert_eq!(
            locations,
            [
                ("opaque", vec![0, 1, 2]),
                ("blended", vec![0, 1, 2]),
                ("moving", vec![0, 1, 2]),
            ]
        );
        // One that returns its own motion is as slangc compiled it.
        let compiled = |path| {
            compile(Path::new(SLANGC), "again", path)
                .expect("compiles")
                .0
        };
        let as_added = |name: &str| {
            let added = adds(&shaders).into_iter().find(|added| added.name == name);
            added.expect("added").spirv.clone()
        };
        let moving_spirv = spirv::decorate_non_uniform(compiled(&moving));
        assert_eq!(as_added("moving"), moving_spirv);
        assert_eq!(spirv::return_no_motion(moving_spirv.clone()), moving_spirv);
    }

    /// The engine's shaders drawing into the game's view without writing depth still return
    /// something at the motion's location: their pipelines don't write it, but lavapipe may, as
    /// `GraphicsDesc::format` in gg-gpu says.
    #[test]
    fn the_engines_shaders_that_draw_into_the_view_return_motion() {
        let shaders: [(&str, &[u8]); 3] = [
            (
                "particles",
                include_bytes!(concat!(env!("OUT_DIR"), "/particles.spv")),
            ),
            ("sky", include_bytes!(concat!(env!("OUT_DIR"), "/sky.spv"))),
            (
                "atmosphere's sky",
                include_bytes!(concat!(env!("OUT_DIR"), "/atmosphere_sky.spv")),
            ),
        ];
        for (name, spirv) in shaders {
            assert_eq!(spirv::fragment_locations(spirv), [0, 1, 2], "{name}");
        }
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
        // Unread parameters of part of a word, which would put every later draw's off their words.
        says(
            refusal::<[f32; 7], [u16; 3]>("vertex_color.slang"),
            &["its type of parameters, [u16; 3], takes 6 bytes, not a whole number of 4-byte"],
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
                "its frame's data, Frame, take 532 bytes, past the 520 the engine's take",
                "it reads brightness from its push constants, past the 128 bytes",
            ],
        );
        says(
            refusal::<Vertex, Color>("float_ids.slang"),
            &["its push constants' ids isn't the address of uint2s"],
        );
        // 8-byte parameters where the IDs go, and IDs where the parameters go, which would pass for
        // each other.
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
                "its fragment stage returns its motion, motion, as what isn't a float4",
                "its fragment stage writes glow at location 3",
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
        // The sprite shader, three model shaders and two games'. Each samples a texture by an
        // instance's index; the model shaders sample the sky's light too, by the frame's indices,
        // uniform across a draw.
        assert_eq!(shaders.changes.len(), 6);
        let non_uniform = |name: &str, spirv: &[u8]| {
            let sampled = spirv::sampled_images(spirv);
            assert!(
                sampled.iter().any(|&(_, decorated, _)| decorated)
                    && (sampled.iter()).all(|&(_, decorated, of)| decorated || !of),
                "{name}: {sampled:?}"
            );
        };
        for added in adds(&shaders) {
            non_uniform(&added.name, &added.spirv);
        }
        // Shading the view from its visibility buffer, each pixel taking its own material.
        let shading = super::non_uniform(crate::mesh::VISIBILITY_SPIRV);
        non_uniform("visibility shading", &shading);
    }

    /// The model shaders load the frame's data whole, unoptimized (`compile` says why): reaching in
    /// for its lights' or materials' address alone makes a pointer to a pointer in buffer memory.
    #[test]
    fn the_model_shaders_load_the_frame_whole() {
        let shaders = [
            ("opaque", model::OPAQUE_SPIRV),
            ("masked", model::MASKED_SPIRV),
            ("masked shadows", model::MASKED_SHADOW_SPIRV),
            ("opaque normals", model::OPAQUE_NORMALS_SPIRV),
            ("masked normals", model::MASKED_NORMALS_SPIRV),
            ("blended", model::BLENDED_SPIRV),
            ("masked visibility", model::MASKED_VISIBILITY_SPIRV),
            (
                "masked visibility",
                model::MASKED_VISIBILITY_PRIMITIVE_SPIRV,
            ),
            ("visibility shading", crate::mesh::VISIBILITY_SPIRV),
        ];
        for (name, spirv) in shaders {
            assert_eq!(spirv::buffer_pointers_to_pointers(spirv), 0, "{name}");
        }
    }

    /// The cull, gather and depth pyramid shaders take every address from push constants (they say
    /// why), none from buffer memory. The model shaders' frame holds some, which the check would
    /// find.
    #[test]
    fn the_cull_shader_holds_no_addresses_in_buffers() {
        let culling = [
            crate::mesh::CULL_SPIRV,
            crate::mesh::GATHER_SPIRV,
            crate::mesh::PYRAMID_SPIRV,
        ];
        for spirv in culling {
            assert_eq!(spirv::buffer_structs_holding_pointers(spirv), 0);
        }
        let frame = spirv::buffer_structs_holding_pointers(model::OPAQUE_SPIRV);
        assert!(frame > 0, "the model shader's frame holds addresses");
    }

    #[test]
    fn a_shader_compiled_again_lists_its_files_and_replaces_itself() {
        let dir = Scratch::new("compiled-again");
        let shaders_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let files = [
            "model_opaque.slang",
            "model.slang",
            "shading.slang",
            "common.slang",
            "mesh.slang",
            "aerial.slang",
            "fog.slang",
        ];
        for file in files {
            fs::copy(shaders_dir.join(file), dir.0.join(file)).expect("a copy");
        }
        let path = dir.0.join("model_opaque.slang");
        let mut shaders = Shaders::default();
        let shader = (shaders.add::<PackedVertex, model::Params>("copy", &path, Blend::Opaque))
            .expect("a shader");
        let [source] = shaders.sources() else {
            panic!("one source");
        };
        assert_eq!((source.name(), source.path()), ("copy", path.as_path()));
        let read = files.map(Some);
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

        // Before a renderer takes the first, the second takes its place; after, it's queued, and
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

        // Removed, it leaves the sources, so it isn't compiled again, and one already compiled from
        // its source is dropped. Removing it again does nothing.
        shaders.changes.clear();
        let compiled = source.compile().expect("the shader compiles again");
        shaders.remove(shader);
        shaders.remove(shader);
        assert!(shaders.sources().is_empty());
        assert!(matches!(shaders.changes[..], [Change::Removed(1)]));
        shaders.replace(compiled);
        assert_eq!(shaders.changes.len(), 1, "the replacement was dropped");
        let found = shaders.find::<PackedVertex, model::Params>(Blend::Opaque, |_| true);
        assert!(found.is_none(), "a removed shader isn't found");

        // A change that doesn't compile, and one that breaks the contract.
        fs::write(&include, "struct Vertex {").expect("the include broken");
        let error = source.compile().expect_err("the include doesn't compile");
        assert_eq!(error.code(), "renderer.bad_shader");
        // The packed vertex, the first struct with a color, a word wider than its type.
        fs::write(&include, &text).expect("the include as it was");
        let shading = dir.0.join("shading.slang");
        let shading_text = fs::read_to_string(&shading).expect("the include's include");
        let wider = shading_text.replacen("uint color;", "uint color;\n    uint extra;", 1);
        assert_ne!(wider, shading_text, "the vertex is wider");
        fs::write(&shading, wider).expect("the include's include changed");
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
        // Names in UTF-8, as slangc writes them on Windows, and one that isn't: on Unix, a name
        // like any other.
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
