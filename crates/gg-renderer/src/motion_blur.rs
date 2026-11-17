//! Motion blur: the view blurred along its motion while the shutter was open, by McGuire et al.'s
//! reconstruction filter (`shaders/motion_blur.slang`).

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec3};
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, PassId};

use crate::taa::Drawn;

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/motion_blur.spv"));

/// Pixels across and down per workgroup, and threads per tile: `GROUP` in
/// `shaders/motion_blur.slang`.
const GROUP: u32 = 8;

/// Tiles down the view's height. A tile's side is the farthest a pixel blurs either way: 20 pixels
/// at 720, as McGuire et al. have it.
const TILES_DOWN: u32 = 36;

/// Motion blur, as [`RenderData::motion_blur`](crate::RenderData::motion_blur) gives it.
///
/// The view is blurred along how far things moved while the shutter was open, centered on where
/// they show. Motion is as [`Antialiasing::Taa`](crate::Antialiasing::Taa) says. A headless frame
/// draws a tick, so its motion is a tick's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionBlur {
    /// The share of the frame the shutter is open, as a film shutter's angle is of a turn: 0 none,
    /// 1 a frame's motion, ½ (film's 180°) by default.
    pub shutter: f32,
}

impl Default for MotionBlur {
    fn default() -> Self {
        Self { shutter: 0.5 }
    }
}

impl MotionBlur {
    pub(crate) fn blurs(&self) -> bool {
        self.shutter.is_finite() && self.shutter > 0.0
    }
}

/// A tile's side in pixels for a view `size` pixels, by [`TILES_DOWN`]: 1 or more.
fn tile(size: [u32; 2]) -> u32 {
    size[1].div_ceil(TILES_DOWN).max(1)
}

/// What the velocity pass reads: `Seen` in `shaders/motion_blur.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Seen {
    world_from_clip: Mat4,
    previous_clip_from_world: Mat4,
    jitter: [f32; 2],
    shutter: f32,
    unused: u32,
}

/// `Push` in `shaders/motion_blur.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct BlurPush {
    seen: u64,
    size: [u32; 2],
    tiles: [u32; 2],
    tile: u32,
    colors: u32,
    depth: u32,
    motion: u32,
    velocity: u32,
    tiled: u32,
    target: u32,
    unused: u32,
}

/// The last frame that blurred the view, and its unjittered world-to-clip matrix.
#[derive(Clone, Copy)]
struct Blurred {
    frame: u64,
    size: [u32; 2],
    clip_from_world: Mat4,
}

pub(crate) struct MotionBlurPass {
    velocity: ComputePipeline,
    tiles: ComputePipeline,
    neighbors: ComputePipeline,
    gather: ComputePipeline,
    last: Option<Blurred>,
}

/// A frame's motion blur in its graph: it reads `drawn` (colors, depth, motion) and writes
/// `blurred`, which the view shows after it. The passes are in order.
pub(crate) struct Plan {
    shutter: f32,
    size: [u32; 2],
    tile: u32,
    tiles: [u32; 2],
    drawn: [ImageId; 3],
    velocity: ImageId,
    tiled: ImageId,
    neighbors: ImageId,
    blurred: ImageId,
    passes: [PassId; 4],
}

impl Plan {
    pub(crate) fn blurred(&self) -> ImageId {
        self.blurred
    }
}

impl MotionBlurPass {
    /// Moves what it keeps of the frame before into the space of a frame whose origin is `by`
    /// further on: what was at `p` is at `p - by`, as
    /// [`RenderData::origin`](crate::RenderData::origin) says.
    pub(crate) fn moved(&mut self, by: Vec3) {
        if let Some(last) = &mut self.last {
            let along = Mat4::from_translation(by);
            last.clip_from_world *= along;
        }
    }

    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        Ok(Self {
            velocity: pipeline("motion blur velocity", c"velocity_main")?,
            tiles: pipeline("motion blur tiles", c"tiles_main")?,
            neighbors: pipeline("motion blur neighbors", c"neighbors_main")?,
            gather: pipeline("motion blur gather", c"gather_main")?,
            last: None,
        })
    }

    /// Declares in `graph` the motion blur, by `blur`, of the view's `drawn` colors, depth and
    /// motion, `size` pixels, into blurred colors of the graph's own.
    pub(crate) fn plan(
        graph: &mut Graph,
        blur: MotionBlur,
        drawn: [ImageId; 3],
        size: [u32; 2],
    ) -> Plan {
        let [colors, depth, motion] = drawn;
        let tile = tile(size);
        let tiles = size.map(|side| side.div_ceil(tile));
        let velocity = graph.hdr("motion blur velocity", size);
        let velocities = graph.compute(label!("motion blur velocity"));
        let velocities = (velocities.sample(depth).sample(motion).overwrite(velocity)).add();
        let tiled = graph.hdr("motion blur tiles", tiles);
        let tiling = graph.compute(label!("motion blur tiles"));
        let tiling = tiling.sample(velocity).overwrite(tiled).add();
        let neighbors = graph.hdr("motion blur neighbors", tiles);
        let neighboring = graph.compute(label!("motion blur neighbors"));
        let neighboring = neighboring.sample(tiled).overwrite(neighbors).add();
        let blurred = graph.hdr("view blurred", size);
        let gather = graph.compute(label!("motion blur"));
        let gather = gather.sample(colors).sample(velocity).sample(neighbors);
        let gather = gather.overwrite(blurred).add();
        Plan {
            shutter: blur.shutter.min(1.0),
            size,
            tile,
            tiles,
            drawn,
            velocity,
            tiled,
            neighbors,
            blurred,
            passes: [velocities, tiling, neighboring, gather],
        }
    }

    /// Records `plan`'s passes against the frame before's view, if it blurred at this size and the
    /// view isn't `cut` to its viewpoint; else as though the camera stood still. Keeps this frame's
    /// view for the next.
    pub(crate) fn record(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &Plan,
        drawn: Drawn,
        cut: bool,
    ) -> Result<()> {
        let number = frame.number();
        let previous = (self.last)
            .filter(|last| !cut && last.frame + 1 == number && last.size == plan.size)
            .map_or(drawn.unjittered, |last| last.clip_from_world);
        let [colors, depth, motion] = plan.drawn;
        let [velocities, tiling, neighboring, gather] = plan.passes;
        let base = BlurPush {
            seen: 0,
            size: plan.size,
            tiles: plan.tiles,
            tile: plan.tile,
            colors: 0,
            depth: 0,
            motion: 0,
            velocity: 0,
            tiled: 0,
            target: 0,
            unused: 0,
        };
        let pixels = |size: [u32; 2]| [size[0].div_ceil(GROUP), size[1].div_ceil(GROUP), 1];
        let mut pass = frame.compute(velocities);
        let seen = Seen {
            world_from_clip: drawn.clip_from_world.inverse(),
            previous_clip_from_world: previous,
            jitter: drawn.jitter.to_array(),
            shutter: plan.shutter,
            unused: 0,
        };
        let push = BlurPush {
            seen: pass.upload(bytemuck::bytes_of(&seen))?.address(),
            depth: pass.sampled(depth),
            motion: pass.sampled(motion),
            target: pass.storage(plan.velocity, 0),
            ..base
        };
        pass.set_pipeline(&self.velocity);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(pixels(plan.size));
        drop(pass);
        let mut pass = frame.compute(tiling);
        let push = BlurPush {
            velocity: pass.sampled(plan.velocity),
            target: pass.storage(plan.tiled, 0),
            ..base
        };
        pass.set_pipeline(&self.tiles);
        pass.push_constants(bytemuck::bytes_of(&push));
        // A workgroup per tile.
        pass.dispatch([plan.tiles[0], plan.tiles[1], 1]);
        drop(pass);
        let mut pass = frame.compute(neighboring);
        let push = BlurPush {
            tiled: pass.sampled(plan.tiled),
            target: pass.storage(plan.neighbors, 0),
            ..base
        };
        pass.set_pipeline(&self.neighbors);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(pixels(plan.tiles));
        drop(pass);
        let mut pass = frame.compute(gather);
        let push = BlurPush {
            colors: pass.sampled(colors),
            velocity: pass.sampled(plan.velocity),
            tiled: pass.sampled(plan.neighbors),
            target: pass.storage(plan.blurred, 0),
            ..base
        };
        pass.set_pipeline(&self.gather);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(pixels(plan.size));
        self.last = Some(Blurred {
            frame: number,
            size: plan.size,
            clip_from_world: drawn.unjittered,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile's side is a 36th of the view's height, rounded up: 20 pixels at 720, at least 1.
    #[test]
    fn tiles_are_a_36th_of_the_views_height() {
        assert_eq!(tile([1280, 720]), 20);
        assert_eq!(tile([1920, 1080]), 30);
        assert_eq!(tile([64, 37]), 2);
        assert_eq!(tile([64, 1]), 1);
    }
}
