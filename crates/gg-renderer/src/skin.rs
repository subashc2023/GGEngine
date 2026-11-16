//! Skinning: a compute pass before the frame draws (`shaders/skin.slang`). It moves the vertices of
//! each skinned mesh in [`Draws::skin`] by its joints' matrices into the frame's own buffer, where
//! the mesh's draws read them instead. Each mesh moves into its own part, header copied first, so
//! draws read it as any mesh's buffer; each separately posed instance is a mesh of its own.
//!
//! A skinned mesh has no meshlets, whose bounds its joints would leave behind, so it draws by its
//! indices, outside the visibility buffer. Its motion is its instance's alone, not its joints'.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::label;
use gg_gpu::{BufferId, ComputePipeline, Device, Frame, Graph, PassId};

use crate::mesh::MeshPass;
use crate::{Draws, PackedVertex};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/skin.spv"));

/// Threads per workgroup of `shaders/skin.slang`, a vertex each.
const GROUP: u32 = 64;

/// The most workgroups across a dispatch on every device. A mesh of more vertices moves in several
/// dispatches.
const MOST_GROUPS: u32 = 65_535;

/// Vertices' room a mesh's header takes: `HEADER_VERTICES` in src/packed.rs.
const HEADER_VERTICES: u64 = 2;

/// Each mesh's part of the buffer starts at a multiple of this many bytes.
const ALIGN: u64 = 16;

/// `Push` in `shaders/skin.slang`.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct SkinPush {
    source: u64,
    target: u64,
    joints: u64,
    vertices: u32,
    joint_count: u32,
    first: u32,
    unused: u32,
}

/// A mesh the frame moves: its handle, buffer, vertex count, joints' place among the frame's, and
/// its part's start in the frame's buffer.
struct Planned {
    mesh: u32,
    source: u64,
    vertices: u32,
    joints: (u32, u32),
    offset: u64,
}

/// What the frame's skin pass moves: its buffer and its pass.
pub(crate) struct SkinPlan {
    pub(crate) buffer: BufferId,
    pass: PassId,
}

/// The pipeline and the frame's plan, kept across frames so it allocates nothing once grown.
pub(crate) struct Skinning {
    pipeline: ComputePipeline,
    planned: Vec<Planned>,
}

impl Skinning {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        Ok(Self {
            pipeline: ComputePipeline::new(device, "skin", SPIRV, c"skin_main")?,
            planned: Vec::new(),
        })
    }

    /// Declares in `graph`, on the graphics queue before the passes that draw, the buffer and pass
    /// that move `draws`' skins of `meshes`' skinned, drawable meshes, in order, if any.
    pub(crate) fn plan(
        &mut self,
        graph: &mut Graph,
        draws: &Draws,
        meshes: &MeshPass,
    ) -> Option<SkinPlan> {
        self.planned.clear();
        let mut bytes = 0;
        for (mesh, joints) in &draws.skins {
            let Some((source, vertices)) = meshes.skinned_mesh(*mesh) else {
                continue;
            };
            let offset = bytes;
            let size = (HEADER_VERTICES + u64::from(vertices)) * size_of::<PackedVertex>() as u64;
            bytes = (offset + size).next_multiple_of(ALIGN);
            let at = |index: usize| u32::try_from(index).expect("fewer than 2^32 joints a frame");
            self.planned.push(Planned {
                mesh: *mesh,
                source,
                vertices,
                joints: (at(joints.start), at(joints.end - joints.start)),
                offset,
            });
        }
        if self.planned.is_empty() {
            return None;
        }
        let buffer = graph.buffer("skinned vertices", bytes);
        let pass = graph.compute(label!("skin")).write_buffer(buffer).add();
        Some(SkinPlan { buffer, pass })
    }

    /// Records `plan`'s pass as the frame's next, moving each planned mesh by its joints' matrices
    /// in `draws`, and has `meshes`' draws read each where it moved. Without a plan, they read
    /// their own.
    pub(crate) fn record(
        &self,
        frame: &mut Frame<'_>,
        plan: Option<&SkinPlan>,
        draws: &Draws,
        meshes: &mut MeshPass,
    ) -> Result<()> {
        let Some(plan) = plan else {
            meshes.set_skinned(std::iter::empty());
            return Ok(());
        };
        let mut pass = frame.compute(plan.pass);
        let base = pass.address(plan.buffer);
        let joints = pass.upload(bytemuck::cast_slice(&draws.joints))?.address();
        pass.set_pipeline(&self.pipeline);
        for planned in &self.planned {
            let (first_joint, joint_count) = planned.joints;
            let mut first = 0;
            while first < planned.vertices {
                let groups = (planned.vertices - first).div_ceil(GROUP).min(MOST_GROUPS);
                let push = SkinPush {
                    source: planned.source,
                    target: base + planned.offset,
                    joints: joints + u64::from(first_joint) * size_of::<[f32; 16]>() as u64,
                    vertices: planned.vertices,
                    joint_count,
                    first,
                    unused: 0,
                };
                pass.push_constants(bytemuck::bytes_of(&push));
                pass.dispatch([groups, 1, 1]);
                first += groups * GROUP;
            }
        }
        drop(pass);
        meshes.set_skinned(
            (self.planned.iter()).map(|planned| (planned.mesh, base + planned.offset)),
        );
        Ok(())
    }
}
