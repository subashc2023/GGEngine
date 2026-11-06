//! Shading the game's view from its visibility buffer, which the meshlets its culling keeps of the
//! runs of the opaque and masked model shaders draw into, into its colors, a pixel at a time, as
//! `shaders/visibility.slang` says.

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::math::Mat4;
use gg_gpu::{Compute, ImageId};

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
    /// range, which it overwrites.
    pub(crate) visibility: ImageId,
    pub(crate) colors: ImageId,
    /// The address of the frame's data, a [`FrameData`](super::FrameData), as the view's draws
    /// read it.
    pub(crate) frame: u64,
}

impl MeshPass {
    /// Records the shading of `view` from its visibility buffer into `pass`, which samples it,
    /// overwrites its colors, and reads what `plan`'s culling kept, as [`CullPlan::shaded`]
    /// declared it: each pixel of its colors as the model shader's fragment would have shaded it,
    /// where a run's meshlets drew, and its clear color elsewhere.
    pub(crate) fn shade(
        &self,
        pass: &mut Compute<'_, '_>,
        plan: &CullPlan,
        view: &Shaded,
    ) -> Result<()> {
        let kept = self.kept(plan, pass);
        let shading = Shading {
            clip_from_world: view.clip_from_world,
            clear: view.clear,
            size: view.size,
            instances: kept.instances,
            params_words: kept.params_words,
            slots: kept.slots,
            meshlets: kept.meshlets,
        };
        let push = ShadePush {
            shading: pass.upload(bytemuck::bytes_of(&shading))?.address(),
            frame: view.frame,
            runs: kept.runs,
            commands: kept.commands,
            transforms: kept.transforms,
            params: kept.params,
            visibility: pass.sampled(view.visibility),
            target: pass.storage(view.colors, 0),
        };
        pass.set_pipeline(&self.shading);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = view.size.map(|side| side.div_ceil(GROUP));
        pass.dispatch([across, down, 1]);
        Ok(())
    }
}
