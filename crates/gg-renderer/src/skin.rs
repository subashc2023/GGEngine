//! Skinning: a compute pass before the frame draws, which moves the vertices of each skinned
//! model's mesh that the frame's [`Draws::skin`] gives, by its joints' matrices, into the frame's
//! own buffer, where the frame's draws of the mesh read them in place of its own, as
//! `shaders/skin.slang` moves them. A mesh moves into a part of that buffer of its own, its header
//! copied first, so that a draw reads it as it reads any mesh's buffer; so each instance of a
//! skinned model, posed apart, is a mesh of its own.
//!
//! A skinned mesh isn't cut into meshlets, whose bounds its joints would leave behind, so it draws
//! by its indices, outside the visibility buffer. Its motion is its instance's alone, not its
//! joints'.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::label;
use gg_gpu::{BufferId, ComputePipeline, Device, Frame, Graph, PassId};

use crate::mesh::MeshPass;
use crate::{Draws, PackedVertex};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/skin.spv"));

/// The threads of a workgroup of `shaders/skin.slang`, a vertex each.
const GROUP: u32 = 64;

/// The most workgroups a dispatch takes across, as every device takes: a mesh of more vertices
/// moves in several dispatches.
const MOST_GROUPS: u32 = 65_535;

/// How many vertices' room a mesh's header takes, as `HEADER_VERTICES` in src/packed.rs.
const HEADER_VERTICES: u64 = 2;

/// Where each mesh's part of the buffer starts, a multiple of this many bytes.
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

/// A mesh the frame moves: its handle, where its buffer is, how many vertices it has, where its
/// joints' matrices are among the frame's, and where its part of the frame's buffer starts.
struct Planned {
    mesh: u32,
    source: u64,
    vertices: u32,
    joints: (u32, u32),
    offset: u64,
}

/// What the frame's skin pass moves, once it's planned: its buffer and its pass.
pub(crate) struct SkinPlan {
    pub(crate) buffer: BufferId,
    pass: PassId,
}

/// The skin pass's pipeline, and what the frame plans, kept from frame to frame so that it
/// allocates nothing once it's grown.
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

    /// Declares in `graph` the buffer and the pass that move `draws`' skins, each of a mesh of
    /// `meshes` that's skinned and drawable, in their order, if there's one: on the graphics
    /// queue, before the passes that draw.
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

    /// Records `plan`'s pass, the next of the frame's, which moves each mesh it planned by its
    /// joints' matrices, which `draws` holds; and has the draws of `meshes` read each where it
    /// moved it. A frame without a plan has them read their own.
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
