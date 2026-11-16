//! Shading the game's view from its visibility buffer, where the opaque and masked model shaders'
//! kept meshlets draw, a pixel at a time (`shaders/visibility.slang`); and first finding its
//! normals there, for reflections.

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::math::Mat4;
use gg_gpu::{Compute, ComputePipeline, ImageId};

use super::MeshPass;
use super::cull::CullPlan;

pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/visibility.spv"));

/// Pixels across and down per workgroup: `GROUP` in `shaders/visibility.slang`.
const GROUP: u32 = 8;

/// What the shading reads of the view and its culling: `Shading` in `shaders/visibility.slang`,
/// field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Shading {
    clip_from_world: Mat4,
    clear: [f32; 4],
    size: [u32; 2],
    instances: u32,
    params_words: u32,
    slots: [u32; 2],
    meshlets: [u32; 2],
    previous: [u32; 2],
    unused: [u32; 2],
}

/// What the shading pushes: `ShadePush` in `shaders/visibility.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct ShadePush {
    shading: u64,
    frame: u64,
    runs: u64,
    commands: u64,
    transforms: u64,
    params: u64,
    visibility: u32,
    target: u32,
    motion: u32,
    unused: u32,
}

/// The game's view, for shading from its visibility buffer.
pub(crate) struct Shaded {
    /// From the world to its clip space, as its passes drew.
    pub(crate) clip_from_world: Mat4,
    /// Linear RGBA where nothing drew.
    pub(crate) clear: [f32; 4],
    /// In pixels.
    pub(crate) size: [u32; 2],
    /// Its visibility buffer, which the pass samples; its HDR colors and motion, which it
    /// overwrites.
    pub(crate) visibility: ImageId,
    pub(crate) colors: ImageId,
    pub(crate) motion: ImageId,
    /// The address of the frame's [`FrameData`](super::FrameData), as the view's draws read it.
    pub(crate) frame: u64,
}

/// What a pass over the game's view's visibility buffer takes of the view.
struct Seen {
    clip_from_world: Mat4,
    clear: [f32; 4],
    size: [u32; 2],
    visibility: ImageId,
    frame: u64,
}

/// The game's view, for finding its normals from its visibility buffer before reflections are
/// traced.
pub(crate) struct Normals {
    /// From the world to its clip space, as its passes drew.
    pub(crate) clip_from_world: Mat4,
    /// In pixels.
    pub(crate) size: [u32; 2],
    /// Its visibility buffer, which the pass samples; its normals, which it overwrites.
    pub(crate) visibility: ImageId,
    pub(crate) normals: ImageId,
    /// The address of the frame's data, materials among it, as what's drawn into the visibility
    /// buffer reads it.
    pub(crate) frame: u64,
}

impl MeshPass {
    /// Records into `pass` the shading of `view` from its visibility buffer, reading what `plan`
    /// kept ([`CullPlan::shaded`]). Where a run's meshlets drew, each pixel's color and motion are
    /// as the model shader's fragment would write them; elsewhere, the clear color and no motion.
    pub(crate) fn shade(
        &self,
        pass: &mut Compute<'_, '_>,
        plan: &CullPlan,
        view: &Shaded,
    ) -> Result<()> {
        let seen = Seen {
            clip_from_world: view.clip_from_world,
            clear: view.clear,
            size: view.size,
            visibility: view.visibility,
            frame: view.frame,
        };
        let targets = [pass.storage(view.colors, 0), pass.storage(view.motion, 0)];
        self.over_visibility(pass, plan, &self.shading, &seen, targets)
    }

    /// Records into `pass` the finding of `view`'s normals from its visibility buffer, reading what
    /// `plan` kept ([`CullPlan::shaded`]). Where a run's meshlets drew, each pixel's world normal
    /// as shading turns it, alpha 1; elsewhere 0.
    pub(crate) fn find_normals(
        &self,
        pass: &mut Compute<'_, '_>,
        plan: &CullPlan,
        view: &Normals,
    ) -> Result<()> {
        let seen = Seen {
            clip_from_world: view.clip_from_world,
            clear: [0.0; 4],
            size: view.size,
            visibility: view.visibility,
            frame: view.frame,
        };
        let targets = [pass.storage(view.normals, 0), 0];
        self.over_visibility(pass, plan, &self.normals, &seen, targets)
    }

    /// Records `pipeline` over `seen`, a pixel per thread, into `pass`, reading what `plan` kept
    /// and writing the storage images at `targets` (target and motion, as `ShadePush` names them).
    fn over_visibility(
        &self,
        pass: &mut Compute<'_, '_>,
        plan: &CullPlan,
        pipeline: &ComputePipeline,
        seen: &Seen,
        [target, motion]: [u32; 2],
    ) -> Result<()> {
        let kept = self.kept(plan, pass);
        let shading = Shading {
            clip_from_world: seen.clip_from_world,
            clear: seen.clear,
            size: seen.size,
            instances: kept.instances,
            params_words: kept.params_words,
            slots: kept.slots,
            meshlets: kept.meshlets,
            previous: kept.previous,
            unused: [0; 2],
        };
        let push = ShadePush {
            shading: pass.upload(bytemuck::bytes_of(&shading))?.address(),
            frame: seen.frame,
            runs: kept.runs,
            commands: kept.commands,
            transforms: kept.transforms,
            params: kept.params,
            visibility: pass.sampled(seen.visibility),
            target,
            motion,
            unused: 0,
        };
        pass.set_pipeline(pipeline);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = seen.size.map(|side| side.div_ceil(GROUP));
        pass.dispatch([across, down, 1]);
        Ok(())
    }
}
