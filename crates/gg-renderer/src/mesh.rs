//! Meshes: the handles the engine draws with, and their vertices and indices in GPU memory.

use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::Arc;

use gg_core::Result;
use gg_core::math::{Mat4, Vec3};
#[cfg(doc)]
use gg_gpu::Format;
use gg_gpu::{ComputePipeline, Device, Faces, GpuBuffer, Pipeline};

use crate::{Draws, shader};

mod cull;
mod draw;
mod pipelines;
mod registry;
mod scene;
mod streaming;
mod visibility;

#[cfg(test)]
pub(crate) use cull::SPIRV as CULL_SPIRV;
pub(crate) use cull::{COUNTS, CullPlan, CullView, FIRST, counts};
#[cfg(test)]
pub(crate) use cull::{GATHER_SPIRV, PYRAMID_SPIRV};
pub(crate) use draw::{FrameData, Push, Runs, Way};
pub(crate) use registry::{Change, Positions, bad_mesh, check, check_lods};
pub use registry::{Lod, Lods, MAX_LEVELS, Mesh, MeshDraw, Meshes};
pub(crate) use scene::Change as SceneChange;
pub use scene::Scene;
use streaming::{Item, MaterialTable, MeshletTable, PlacedMeshlets, StandIns, TextureSlot};
#[cfg(test)]
pub(crate) use visibility::SPIRV as VISIBILITY_SPIRV;
pub(crate) use visibility::{Normals, Shaded};

/// Draws meshes with their shaders into the game's view ([`Format::HDR`]), writing IDs and testing
/// depth. It streams meshes and the textures shaders sample to the GPU within a byte budget per
/// frame, and keeps them there.
pub(crate) struct MeshPass {
    device: Arc<Device>,
    /// By handle; `None` for one removed.
    shaders: Vec<Option<GpuShader>>,
    /// By handle, once drawable.
    meshes: Vec<Held<GpuMesh>>,
    /// By handle, which is also its slot in the bindless sets.
    textures: Vec<TextureSlot>,
    /// What slots show until their images arrive. Dropped after `textures`, whose slots show them.
    stand_ins: StandIns,
    /// What's streaming, in the order added, each until it's drawable or removed. It keeps its
    /// memory, so a frame with nothing to stream allocates nothing.
    streaming: VecDeque<Item>,
    /// The materials models' draws index.
    materials: MaterialTable,
    /// The most bytes a frame copies ([`Settings::upload_budget`](crate::Settings::upload_budget)),
    /// but for the smallest whole part.
    budget: u64,
    /// Culls the frame's opaque runs, and keeps what it culled.
    culling: cull::Culling,
    /// The scene's instances, as the GPU keeps them.
    scene: scene::Kept,
    /// Culls the meshlets of the meshes that have them.
    meshlet_table: MeshletTable,
    /// Shades the view from its visibility buffer; `normals` finds its normals there for
    /// reflections.
    shading: ComputePipeline,
    normals: ComputePipeline,
    /// By mesh handle, where this frame's skin pass moved its vertices, which its draws read
    /// instead; 0 if it didn't. Kept across frames, so it allocates nothing once grown.
    skinned: Vec<u64>,
    /// The meshes `skinned` holds an address for.
    skinned_set: Vec<u32>,
    /// Whether the last frame's uploads changed what's drawn: a mesh, texture or material came,
    /// changed or went.
    uploaded: bool,
}

struct GpuShader {
    pipeline: Pipeline,
    /// Draws its depth alone, if it writes depth: its vertex shader, and `shadow_main` if its
    /// shadows are cut out ([`shader::Added::shadow`]).
    depth_only: Option<DepthOnly>,
    /// For a model shader: draws a view's kept meshlets, and their depth alone. Through mesh
    /// shaders where the device has them, else vertex shaders ([`shader::Added::meshlets`]).
    meshlets: Option<(Pipeline, Option<DepthOnly>)>,
    /// Draws them into the visibility buffer, likewise.
    visibility: Option<Pipeline>,
    /// Labels its runs of draws in captures.
    label: CString,
    vertex_size: usize,
    params_size: usize,
    /// As [`shader::writes_depth`] says.
    writes_depth: bool,
}

/// Draws a shader's depth alone: into a shadow map, clamped to the map's near side and set back by
/// slope; and into the depth the view's ambient occlusion takes. For a model shader (with
/// `normal_main`), also draws its normals for reflections, where it's nearest in that depth.
struct DepthOnly {
    shadow: Pipeline,
    view: Pipeline,
    normals: Option<Pipeline>,
}

struct GpuMesh {
    vertices: GpuBuffer,
    /// Every level's indices.
    indices: GpuBuffer,
    /// Where each level is in `indices`, and the sphere that picks among them. A mesh added without
    /// levels has one: all its indices.
    lods: Lods,
    vertex_size: usize,
    /// Where its meshlets are in the table, for a model's mesh that has them.
    meshlets: Option<PlacedMeshlets>,
    /// For a skinned model's mesh, its vertex count, which the skin pass moves.
    skinned: Option<u32>,
    /// For an unskinned model's mesh, its positions, which the path tracer reads in place.
    positions: Option<Positions>,
}

/// What the pass holds for a mesh handle: nothing while it streams, what it draws with, or nothing
/// once removed.
enum Held<T> {
    Waiting,
    Ready(T),
    Removed,
}

impl<T> Held<T> {
    fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(ready) => Some(ready),
            Self::Waiting | Self::Removed => None,
        }
    }
}

impl MeshPass {
    pub(crate) fn new(device: &Arc<Device>, budget: u64) -> Result<Self> {
        let from_visibility = shader::non_uniform(visibility::SPIRV);
        let pipeline = |name, entry| ComputePipeline::new(device, name, &from_visibility, entry);
        Ok(Self {
            device: device.clone(),
            shaders: Vec::new(),
            meshes: Vec::new(),
            textures: Vec::new(),
            stand_ins: StandIns::new(device)?,
            streaming: VecDeque::new(),
            materials: MaterialTable {
                table: None,
                entries: Vec::new(),
            },
            budget,
            culling: cull::Culling::new(device)?,
            shading: pipeline("view shading", c"shade_main")?,
            normals: pipeline("view normals", c"normal_main")?,
            scene: scene::Kept::default(),
            meshlet_table: MeshletTable {
                table: None,
                entries: Vec::new(),
                written: 0,
            },
            skinned: Vec::new(),
            skinned_set: Vec::new(),
            uploaded: false,
        })
    }

    /// Skinned mesh `index`'s vertices and their count, if drawable: its buffer's address, which
    /// starts with its header, as its own draws read it.
    pub(crate) fn skinned_mesh(&self, index: u32) -> Option<(u64, u32)> {
        let mesh = self.meshes.get(index as usize)?.ready()?;
        Some((mesh.vertices.address(), mesh.skinned?))
    }

    /// Has this frame's draws of each mesh in `moved` read its vertices at the address beside it
    /// (starting with its header), where the skin pass moved them; the rest read their own.
    pub(crate) fn set_skinned(&mut self, moved: impl Iterator<Item = (u32, u64)>) {
        for index in self.skinned_set.drain(..) {
            self.skinned[index as usize] = 0;
        }
        for (index, address) in moved {
            let at = index as usize;
            if self.skinned.len() <= at {
                self.skinned.resize(at + 1, 0);
            }
            self.skinned[at] = address;
            self.skinned_set.push(index);
        }
    }

    /// Where mesh `index`'s draws read its vertices this frame: where the skin pass moved them, or
    /// its own buffer.
    fn vertices_address(&self, index: u32, mesh: &GpuMesh) -> u64 {
        match self.skinned.get(index as usize) {
            Some(&moved) if moved != 0 => moved,
            _ => mesh.vertices.address(),
        }
    }

    /// Moves the scene's records into a frame whose origin is `by` past the last frame's
    /// ([`Kept::moved`](scene::Kept::moved)).
    pub(crate) fn moved(&mut self, by: Vec3) {
        self.scene.moved(by);
    }

    /// Whether the last frame's uploads changed what's drawn: a mesh, texture or material came,
    /// changed or went.
    pub(crate) fn uploaded(&self) -> bool {
        self.uploaded
    }

    /// What the path tracer reads of drawable mesh `index`, if it's an unskinned model's.
    pub(crate) fn traced_mesh(&self, index: u32) -> Option<TracedMesh<'_>> {
        let mesh = self.meshes.get(index as usize)?.ready()?;
        let level = mesh.lods.levels.first()?;
        Some(TracedMesh {
            vertices: &mesh.vertices,
            indices: &mesh.indices,
            first_index: level.start,
            triangles: level.count / 3,
            positions: mesh.positions?,
        })
    }

    /// Puts into `out` the instances the path tracer traces: the scene's, then `draws`', of the
    /// engine's opaque and masked model shaders. A mesh not yet drawable is in, which
    /// [`traced_mesh`](Self::traced_mesh) leaves out.
    pub(crate) fn traced_instances(&self, draws: &Draws, out: &mut Vec<TracedInstance>) {
        out.clear();
        let traced = |shader: u32| {
            (self.shaders.get(shader as usize))
                .and_then(Option::as_ref)
                .filter(|shader| shader.meshlets.is_some())
        };
        for (run, transform, material) in self.scene.instances() {
            if traced(run.shader).is_some() {
                out.push(TracedInstance {
                    mesh: run.mesh,
                    transform,
                    material,
                    both_faces: run.faces == Faces::Both,
                });
            }
        }
        for run in draws.runs.iter().filter(|run| run.particles.is_none()) {
            let Some(shader) = traced(run.shader) else {
                continue;
            };
            let params = &draws.params[run.params.clone()];
            for (i, &transform) in draws.transforms[run.instances.clone()].iter().enumerate() {
                let at = i * shader.params_size;
                let material = bytemuck::pod_read_unaligned(&params[at..at + 4]);
                out.push(TracedInstance {
                    mesh: run.mesh,
                    transform,
                    material,
                    both_faces: run.faces == Faces::Both,
                });
            }
        }
    }
}

/// What the path tracer reads of a drawable model mesh: its buffers, its level 0's indices, and its
/// positions.
pub(crate) struct TracedMesh<'a> {
    pub(crate) vertices: &'a GpuBuffer,
    pub(crate) indices: &'a GpuBuffer,
    pub(crate) first_index: u32,
    pub(crate) triangles: u32,
    pub(crate) positions: Positions,
}

/// An instance the path tracer traces: its mesh, transform, material, and whether rays find its
/// back faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TracedInstance {
    pub(crate) mesh: u32,
    pub(crate) transform: Mat4,
    pub(crate) material: u32,
    pub(crate) both_faces: bool,
}

#[cfg(test)]
mod tests;
