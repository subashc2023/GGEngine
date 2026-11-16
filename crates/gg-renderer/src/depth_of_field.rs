//! Depth of field: a thin lens's blur, gathered at half size into near and far fields and
//! composited by their circles of confusion (`shaders/depth_of_field.slang`).

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::libm::tanf;
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, PassId};

use crate::{Projection, Viewpoint};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/depth_of_field.spv"));

/// Texels across and down per workgroup: `GROUP` in `shaders/depth_of_field.slang`.
const GROUP: u32 = 8;

/// Texels at half size per tile, each way: `TILE` in `shaders/depth_of_field.slang`.
const TILE: u32 = 8;

/// The sensor height in meters: full frame's 24 mm, which focal lengths and f-numbers are quoted
/// for.
const SENSOR: f32 = 0.024;

/// The largest circle's radius, as a share of the view's height. Past it the gather's rings can't
/// blend.
const LARGEST: f32 = 0.02;

/// Depth of field, as [`RenderData::depth_of_field`](crate::RenderData::depth_of_field) gives it: a
/// thin lens of the focal length the field of view takes on a 24 mm sensor. World units are meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthOfField {
    /// The focus's distance ahead, in world units: 3 by default. At least twice the focal length, a
    /// lens's closest.
    pub focus_distance: f32,
    /// Focal length over aperture: 1.4 by default, a fast lens's widest. At 50° focused at 3, the
    /// far blur's radius is a third of a percent of the view's height; each stop (×√2) divides it
    /// by √2.
    pub f_number: f32,
}

impl Default for DepthOfField {
    fn default() -> Self {
        Self {
            focus_distance: 3.0,
            f_number: 1.4,
        }
    }
}

impl DepthOfField {
    fn blurs(&self) -> bool {
        let positive = |value: f32| value.is_finite() && value > 0.0;
        positive(self.focus_distance) && positive(self.f_number)
    }
}

/// The circle's radius in pixels at reversed depth `z`: `add + mul × z`, positive past the focus,
/// within `largest`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Circle {
    add: f32,
    mul: f32,
    largest: f32,
}

impl Circle {
    /// The circles of a view `size` pixels from `viewpoint`: none without perspective (no lens) or
    /// a focus that doesn't blur.
    ///
    /// A thin lens of focal length `f` and aperture `A = f / N` focused at `S` spreads a point at
    /// distance `d` over a circle `A f (d - S) / (d (S - f))` across on its sensor. Reversed depth
    /// is `near / d`, so the radius is linear in it: `K (1 - S z / near)`, `K` the radius at
    /// infinity, `A f / (S - f) / 2`, in the view's pixels.
    pub(crate) fn new(focus: DepthOfField, viewpoint: &Viewpoint, size: [u32; 2]) -> Option<Self> {
        let Projection::Perspective { fov, near } = viewpoint.projection else {
            return None;
        };
        if !focus.blurs() {
            return None;
        }
        let height = size[1] as f32;
        let focal = SENSOR * 0.5 / tanf(fov * 0.5);
        let aperture = focal / focus.f_number;
        let distance = focus.focus_distance.max(2.0 * focal);
        let farthest = aperture * focal / (distance - focal) * 0.5 * height / SENSOR;
        Some(Self {
            add: farthest,
            mul: -farthest * distance / near,
            largest: LARGEST * height,
        })
    }

    /// Tiles a dilation reaches each way: the largest circle's span at half size.
    fn reach(&self) -> u32 {
        (self.largest * 0.5 / TILE as f32).ceil() as u32
    }
}

/// `Push` in `shaders/depth_of_field.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct DepthOfFieldPush {
    size: [u32; 2],
    halved_size: [u32; 2],
    tiles: [u32; 2],
    colors: u32,
    depth: u32,
    halved: u32,
    tiled: u32,
    far: u32,
    near: u32,
    target: u32,
    add: f32,
    mul: f32,
    largest: f32,
    reach: u32,
}

pub(crate) struct DepthOfFieldPass {
    downsample: ComputePipeline,
    tiles: ComputePipeline,
    dilate: ComputePipeline,
    gather: ComputePipeline,
    composite: ComputePipeline,
}

/// A frame's depth of field in its graph. It reads `depth`, reads then writes `colors`, and owns
/// the other images (`shaders/depth_of_field.slang`). The passes are in order.
pub(crate) struct Plan {
    circle: Circle,
    size: [u32; 2],
    halved_size: [u32; 2],
    tiles: [u32; 2],
    colors: ImageId,
    depth: ImageId,
    halved: ImageId,
    tiled: ImageId,
    dilated: ImageId,
    far: ImageId,
    near: ImageId,
    passes: [PassId; 5],
}

impl DepthOfFieldPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        Ok(Self {
            downsample: pipeline("depth of field downsample", c"downsample_main")?,
            tiles: pipeline("depth of field tiles", c"tiles_main")?,
            dilate: pipeline("depth of field dilate", c"dilate_main")?,
            gather: pipeline("depth of field gather", c"gather_main")?,
            composite: pipeline("depth of field composite", c"composite_main")?,
        })
    }

    /// Declares in `graph` the depth of field of the view's HDR `colors`, `size` pixels, from its
    /// `depth`, in place.
    pub(crate) fn plan(
        graph: &mut Graph,
        circle: Circle,
        colors: ImageId,
        depth: ImageId,
        size: [u32; 2],
    ) -> Plan {
        let halved_size = size.map(|side| side.div_ceil(2));
        let tiles = halved_size.map(|side| side.div_ceil(TILE));
        let halved = graph.hdr("depth of field halved", halved_size);
        let downsample = graph.compute(label!("depth of field downsample"));
        let downsample = (downsample.sample(colors).sample(depth).overwrite(halved)).add();
        let tiled = graph.hdr("depth of field tiles", tiles);
        let tile = graph.compute(label!("depth of field tiles"));
        let tile = tile.sample(halved).overwrite(tiled).add();
        let dilated = graph.hdr("depth of field dilated", tiles);
        let dilate = graph.compute(label!("depth of field dilate"));
        let dilate = dilate.sample(tiled).overwrite(dilated).add();
        let far = graph.hdr("depth of field far", halved_size);
        let near = graph.hdr("depth of field near", halved_size);
        let gather = graph.compute(label!("depth of field gather"));
        let gather = gather.sample(halved).sample(dilated);
        let gather = gather.overwrite(far).overwrite(near).add();
        let composite = graph.compute(label!("depth of field"));
        let composite = composite.sample(depth).sample(far).sample(near);
        let composite = composite.write(colors).add();
        Plan {
            circle,
            size,
            halved_size,
            tiles,
            colors,
            depth,
            halved,
            tiled,
            dilated,
            far,
            near,
            passes: [downsample, tile, dilate, gather, composite],
        }
    }

    pub(crate) fn record(&self, frame: &mut Frame<'_>, plan: &Plan) {
        let Circle { add, mul, largest } = plan.circle;
        let base = DepthOfFieldPush {
            size: plan.size,
            halved_size: plan.halved_size,
            tiles: plan.tiles,
            colors: 0,
            depth: 0,
            halved: 0,
            tiled: 0,
            far: 0,
            near: 0,
            target: 0,
            add,
            mul,
            largest,
            reach: plan.circle.reach(),
        };
        let [downsample, tiles, dilate, gather, composite] = plan.passes;
        let groups =
            |size: [u32; 2], group: u32| [size[0].div_ceil(group), size[1].div_ceil(group), 1];
        let mut pass = frame.compute(downsample);
        let push = DepthOfFieldPush {
            colors: pass.sampled(plan.colors),
            depth: pass.sampled(plan.depth),
            target: pass.storage(plan.halved, 0),
            ..base
        };
        pass.set_pipeline(&self.downsample);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(groups(plan.halved_size, GROUP));
        drop(pass);
        let mut pass = frame.compute(tiles);
        let push = DepthOfFieldPush {
            halved: pass.sampled(plan.halved),
            target: pass.storage(plan.tiled, 0),
            ..base
        };
        pass.set_pipeline(&self.tiles);
        pass.push_constants(bytemuck::bytes_of(&push));
        // A workgroup per tile.
        pass.dispatch([plan.tiles[0], plan.tiles[1], 1]);
        drop(pass);
        let mut pass = frame.compute(dilate);
        let push = DepthOfFieldPush {
            tiled: pass.sampled(plan.tiled),
            target: pass.storage(plan.dilated, 0),
            ..base
        };
        pass.set_pipeline(&self.dilate);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(groups(plan.tiles, GROUP));
        drop(pass);
        let mut pass = frame.compute(gather);
        let push = DepthOfFieldPush {
            halved: pass.sampled(plan.halved),
            tiled: pass.sampled(plan.dilated),
            far: pass.storage(plan.far, 0),
            near: pass.storage(plan.near, 0),
            ..base
        };
        pass.set_pipeline(&self.gather);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(groups(plan.halved_size, GROUP));
        drop(pass);
        let mut pass = frame.compute(composite);
        let push = DepthOfFieldPush {
            colors: pass.storage(plan.colors, 0),
            depth: pass.sampled(plan.depth),
            far: pass.sampled(plan.far),
            near: pass.sampled(plan.near),
            ..base
        };
        pass.set_pipeline(&self.composite);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(groups(plan.size, GROUP));
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{DVec3, Quat};

    use super::*;

    fn seen(fov: f32, near: f32) -> Viewpoint {
        Viewpoint {
            position: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            projection: Projection::Perspective { fov, near },
        }
    }

    /// A thin lens's circle: none at the focus, half the farthest's at twice its distance,
    /// unbounded nearer but for the largest. The farthest's is `A f / (S - f)` on a 24 mm sensor,
    /// doubling at half the f-number. None without perspective or a blurring focus.
    #[test]
    fn circles_are_a_thin_lenss() {
        let focus = DepthOfField {
            focus_distance: 4.0,
            f_number: 2.0,
        };
        // A field of view whose focal length is 50 mm, as tan(fov / 2) = 12 / 50.
        let fov = 2.0 * gg_core::math::libm::atanf(12.0 / 50.0);
        let size = [1280, 720];
        let circle = Circle::new(focus, &seen(fov, 0.1), size).expect("blurs");
        // At distance d, reversed depth is near / d.
        let at =
            |d: f32| (circle.add + circle.mul * 0.1 / d).clamp(-circle.largest, circle.largest);
        let farthest = 0.025 * 0.05 / (4.0 - 0.05) * 0.5 * 720.0 / 0.024;
        assert!(
            (at(f32::INFINITY) - farthest).abs() < 1e-4,
            "{}",
            at(f32::INFINITY)
        );
        assert!(at(4.0).abs() < 1e-4, "{}", at(4.0));
        assert!((at(8.0) - farthest * 0.5).abs() < 1e-4, "{}", at(8.0));
        assert!((at(2.0) + farthest).abs() < 1e-4, "{}", at(2.0));
        assert_eq!(at(0.1), -0.02 * 720.0);
        let wider = DepthOfField {
            f_number: 1.0,
            ..focus
        };
        let wide = Circle::new(wider, &seen(fov, 0.1), size).expect("blurs");
        assert!((wide.add - 2.0 * circle.add).abs() < 1e-4);
        let flat = Viewpoint::default();
        assert_eq!(Circle::new(focus, &flat, size), None);
        let still = DepthOfField {
            f_number: 0.0,
            ..focus
        };
        assert_eq!(Circle::new(still, &seen(fov, 0.1), size), None);
        // A focus nearer than twice the focal length is taken as that, short of infinite circles.
        let close = DepthOfField {
            focus_distance: 0.0001,
            ..focus
        };
        let closest = Circle::new(close, &seen(fov, 0.1), size).expect("blurs");
        assert!(closest.add.is_finite() && closest.mul.is_finite());
    }
}
