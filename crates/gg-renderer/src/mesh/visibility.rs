//! Shading the game's view from its visibility buffer, which the meshlets its culling keeps of the
//! runs of the opaque and masked model shaders draw into, into its colors, a pixel at a time, as
//! `shaders/visibility.slang` says; and, for its reflections, finding its normals from it first.

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::math::Mat4;
use gg_gpu::{Compute, ComputePipeline, ImageId};

use super::MeshPass;
use super::cull::CullPlan;

pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/visibility.spv"));

/// How many pixels across and down a workgroup takes: `GROUP` in `shaders/visibility.slang`.
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

/// The game's view, as its shading from its visibility buffer takes it.
pub(crate) struct Shaded {
    /// From the world to its clip space, as its passes drew.
    pub(crate) clip_from_world: Mat4,
    /// What it clears to, in linear RGBA, where nothing drew.
    pub(crate) clear: [f32; 4],
    /// Its size in pixels.
    pub(crate) size: [u32; 2],
    /// Its visibility buffer, which the shading's pass samples, and its colors of high dynamic
    /// range and its motion, which it overwrites.
    pub(crate) visibility: ImageId,
    pub(crate) colors: ImageId,
    pub(crate) motion: ImageId,
    /// The address of the frame's data, a [`FrameData`](super::FrameData), as the view's draws
    /// read it.
    pub(crate) frame: u64,
}

/// What a pass over the game's view's visibility buffer takes of the view: from the world to its
/// clip space, what it clears to where nothing drew, its size, its visibility buffer, and the
/// address of the frame's data.
struct Seen {
    clip_from_world: Mat4,
    clear: [f32; 4],
    size: [u32; 2],
    visibility: ImageId,
    frame: u64,
}

/// The game's view, as finding its normals from its visibility buffer takes it, before its
/// reflections are traced.
pub(crate) struct Normals {
    /// From the world to its clip space, as its passes drew.
    pub(crate) clip_from_world: Mat4,
    /// Its size in pixels.
    pub(crate) size: [u32; 2],
    /// Its visibility buffer, which the pass samples, and its normals, which it overwrites.
    pub(crate) visibility: ImageId,
    pub(crate) normals: ImageId,
    /// The address of the frame's data, as what's drawn into the visibility buffer reads it, its
    /// materials among it.
    pub(crate) frame: u64,
}

impl MeshPass {
    /// Records the shading of `view` from its visibility buffer into `pass`, which samples it,
    /// overwrites its colors and its motion, and reads what `plan`'s culling kept, as
    /// [`CullPlan::shaded`] declared it: each pixel of its colors, and of its motion, as the model
    /// shader's fragment would have written it, where a run's meshlets drew, and its clear color,
    /// and no motion, elsewhere.
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

    /// Records the finding of `view`'s normals from its visibility buffer into `pass`, which
    /// samples it, overwrites its normals, and reads what `plan`'s culling kept, as
    /// [`CullPlan::shaded`] declared it: each pixel's normal in the world, as the view's shading
    /// turns it, and 1 in alpha, where a run's meshlets drew, and 0 elsewhere.
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

    /// Records `pipeline` over `seen`, a pixel a thread, into `pass`, which reads what `plan`'s
    /// culling kept, writing the storage images at `targets`, its target and its motion, as
    /// `ShadePush` names them.
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
