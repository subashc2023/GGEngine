//! Meshes: the handles that the engine draws with, and the vertices and indices behind them in
//! GPU memory, drawn with their shaders and textures, games' and the engine's, in the game's view,
//! as its camera sees them.

use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::Arc;

use gg_core::Result;
use gg_core::math::Vec3;
#[cfg(doc)]
use gg_gpu::Format;
use gg_gpu::{ComputePipeline, Device, GpuBuffer, Pipeline};

use crate::shader;

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
pub(crate) use registry::{Change, bad_mesh, check, check_lods};
pub use registry::{Lod, Lods, MAX_LEVELS, Mesh, MeshDraw, Meshes};
pub(crate) use scene::Change as SceneChange;
pub use scene::Scene;
use streaming::{Item, MaterialTable, MeshletTable, PlacedMeshlets, StandIns, TextureSlot};
#[cfg(test)]
pub(crate) use visibility::SPIRV as VISIBILITY_SPIRV;
pub(crate) use visibility::{Normals, Shaded};

/// Draws meshes with their shaders into the game's view, colors of high dynamic range of
/// [`Format::HDR`], writing their IDs beside them and testing its depth image, streams the
/// meshes' vertices and indices and the textures the shaders sample to the GPU within a budget of
/// bytes each frame, and keeps them there.
pub(crate) struct MeshPass {
    device: Arc<Device>,
    /// Each shader's pipeline, by its handle; `None` for one removed.
    shaders: Vec<Option<GpuShader>>,
    /// Each mesh's buffers, by its handle, once they're drawable.
    meshes: Vec<Held<GpuMesh>>,
    /// Each texture, by its handle, which is its slot in the bindless sets too.
    textures: Vec<TextureSlot>,
    /// What the renderer's slots show until their textures' images are there: dropped after the
    /// textures, whose slots show them.
    stand_ins: StandIns,
    /// What's being streamed to the GPU, in the order it was added, each until it's drawable or
    /// removed; it keeps its memory, so that a frame with nothing to stream allocates nothing.
    streaming: VecDeque<Item>,
    /// The materials models' draws index.
    materials: MaterialTable,
    /// The most bytes a frame copies, as [`Settings::upload_budget`](crate::Settings::upload_budget)
    /// has it, but for the smallest whole part.
    budget: u64,
    /// What culls the frame's opaque runs, and what it culled.
    culling: cull::Culling,
    /// The scene's instances, as the GPU keeps them.
    scene: scene::Kept,
    /// What culls the meshlets of the meshes drawn by them.
    meshlet_table: MeshletTable,
    /// What shades the game's view from its visibility buffer, and what finds its normals from it
    /// for its reflections.
    shading: ComputePipeline,
    normals: ComputePipeline,
    /// Where this frame's skin pass moved each skinned mesh's vertices, by the mesh's handle, which
    /// its draws read in place of its own; 0 for a mesh it didn't move. Kept from frame to frame,
    /// each frame setting again those it sets, so that it allocates nothing once it's grown.
    skinned: Vec<u64>,
    /// The meshes `skinned` holds an address for.
    skinned_set: Vec<u32>,
}

struct GpuShader {
    pipeline: Pipeline,
    /// What draws its depth alone, if it writes depth: its vertex shader, and `shadow_main` if its
    /// shadows are cut out ([`shader::Added::shadow`]).
    depth_only: Option<DepthOnly>,
    /// What draws the meshlets a view keeps of a mesh, and their depth alone, for a model shader
    /// that views draw meshes with meshlets by: through mesh shaders where the device has them, and
    /// through vertex shaders elsewhere ([`shader::Added::meshlets`]).
    meshlets: Option<(Pipeline, Option<DepthOnly>)>,
    /// What draws them into the visibility buffer, for such a shader, likewise.
    visibility: Option<Pipeline>,
    /// Its name, which captures label its runs of draws with.
    label: CString,
    /// The size of the vertices it reads.
    vertex_size: usize,
    /// The size of the parameters it takes.
    params_size: usize,
    /// Whether the pipeline writes depth, as [`shader::writes_depth`] says.
    writes_depth: bool,
}

/// What draws a shader's depth alone, by its geometry: into a shadow map, its depth clamped to the
/// map's near side and set back by its slope; and into the depth the view's ambient occlusion
/// takes, as the view draws it. And for a model shader, which has `normal_main`, what draws its
/// normals, for the view's reflections, where it's nearest in that depth, once it's drawn.
struct DepthOnly {
    shadow: Pipeline,
    view: Pipeline,
    normals: Option<Pipeline>,
}

struct GpuMesh {
    vertices: GpuBuffer,
    /// Every level's indices.
    indices: GpuBuffer,
    /// Where each level is in `indices`, and the sphere that picks among them: a mesh added
    /// without levels has one, all its indices.
    lods: Lods,
    vertex_size: usize,
    /// Where its meshlets are in the table, for a model's mesh that has them.
    meshlets: Option<PlacedMeshlets>,
    /// For a skinned model's mesh, how many vertices it has, which the skin pass moves.
    skinned: Option<u32>,
}

/// What the pass holds for a mesh's handle: nothing yet, while what was added streams, what it
/// draws with, or nothing again, once removed.
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
    /// A pass whose frames copy at most `budget` bytes each.
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
        })
    }

    /// The vertices of skinned mesh `index`, and how many there are, if it's drawable: the
    /// address of its buffer, which starts with its header, as its own draws read it.
    pub(crate) fn skinned_mesh(&self, index: u32) -> Option<(u64, u32)> {
        let mesh = self.meshes.get(index as usize)?.ready()?;
        Some((mesh.vertices.address(), mesh.skinned?))
    }

    /// Has the frame's draws of each of `moved`'s meshes, by its handle, read its vertices where
    /// the skin pass moved them, at the address beside it, which starts with its header; and of
    /// the rest, theirs.
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

    /// Where mesh `index`, `mesh`, has its draws read its vertices this frame: where the skin pass
    /// moved them, if it did, or else its own buffer.
    fn vertices_address(&self, index: u32, mesh: &GpuMesh) -> u64 {
        match self.skinned.get(index as usize) {
            Some(&moved) if moved != 0 => moved,
            _ => mesh.vertices.address(),
        }
    }

    /// Takes the scene's records into the space of a frame whose origin is `by` further along than
    /// the frame before's, as [`Kept::moved`](scene::Kept::moved) says.
    pub(crate) fn moved(&mut self, by: Vec3) {
        self.scene.moved(by);
    }
}

#[cfg(test)]
mod tests;
