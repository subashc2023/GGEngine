//! Clustered lighting: the game's view cut into clusters, tiles across and down it by slices along
//! its depth, and the frame's lights binned into them on the GPU, each cluster's the lights whose
//! reach meets it, so that each lit fragment shades by its cluster's lights rather than by every
//! light, as `shaders/clusters.slang` bins them and `shaders/model.slang` reads them (Olsson,
//! Billeter and Assarsson, "Clustered Deferred and Forward Shading", HPG 2012, whose slices grow
//! exponentially along the depth, as these do with perspective); and the frame's lights in the
//! order shaders read them.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::label;
use gg_core::math::{Mat4, Vec3, Vec4, libm};
use gg_gpu::{BufferId, ComputePipeline, Device, Frame, Graph, PassId};

use crate::light::GpuLight;
use crate::{Light, Projection, Viewpoint};

pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/clusters.spv"));

/// The view's tiles across and down, and its slices along its depth.
const ACROSS: u32 = 16;
const DOWN: u32 = 9;
const SLICES: u32 = 24;
const CLUSTERS: u32 = ACROSS * DOWN * SLICES;

/// The frame's lights as shaders read them, and the pipeline that bins them into the clusters.
pub(crate) struct Lighting {
    binning: ComputePipeline,
    /// Those that light everywhere first, as [`Light::everywhere`] says, then the rest, each in the
    /// order the frame gives them, kept between frames, so that one with no more than the last
    /// allocates nothing.
    lights: Vec<GpuLight>,
    /// How many of them light everywhere.
    everywhere: u32,
    /// How far ahead of the view the farthest of the rest reaches, which a perspective's slices
    /// end at.
    reach: f32,
}

/// The clusters' buffer and the pass that copies the lights to its front and bins them into it, in
/// a frame's graph.
pub(crate) struct ClusterPlan {
    pub(crate) buffer: BufferId,
    pass: PassId,
    /// Each cluster's words, a light binned a bit.
    words: u32,
    /// The lights' copies' words, after which the clusters' start.
    bits: u32,
}

/// What shaders read of the clusters, through the frame's data: `Clusters` in `shaders/mesh.slang`,
/// field for field, which says what each is. Its default has none, so that every fragment shades
/// by every light.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, NoUninit)]
pub(crate) struct ClusterData {
    everywhere: u32,
    words: u32,
    bits: u32,
    x: Vec4,
    y: Vec4,
    w: Vec4,
    depth: Vec4,
    slice_scale: f32,
    slice_bias: f32,
    perspective: u32,
    across: u32,
    down: u32,
    slices: u32,
    unused: u32,
}

/// What the binning pushes, as `BinPush` in `shaders/clusters.slang` has it.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct BinPush {
    right: Vec4,
    up: Vec4,
    ahead: Vec4,
    lights: u64,
    copies: u64,
    clusters: u64,
    first: u32,
    count: u32,
    words: u32,
    across: u32,
    down: u32,
    slices: u32,
    x_scale: f32,
    y_scale: f32,
    slice_scale: f32,
    slice_bias: f32,
    perspective: u32,
    light_words: u32,
}

impl Lighting {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        Ok(Self {
            binning: ComputePipeline::new(device, "light clusters", SPIRV, c"bin_main")?,
            lights: Vec::new(),
            everywhere: 0,
            reach: 0.0,
        })
    }

    /// Puts `lights` in the order shaders read them, and finds how far ahead of `viewpoint` those
    /// that are binned reach.
    ///
    /// # Panics
    ///
    /// If there are 2^32 lights or more.
    pub(crate) fn order(&mut self, lights: &[Light], viewpoint: &Viewpoint) {
        let _zone = gg_core::zone!();
        self.lights.clear();
        let everywhere = lights.iter().filter(|light| light.everywhere());
        self.lights.extend(everywhere.map(GpuLight::new));
        self.everywhere = u32::try_from(self.lights.len()).expect("fewer than 2^32 lights");
        let ahead = viewpoint.rotation * Vec3::NEG_Z;
        self.reach = f32::MIN;
        for light in lights.iter().filter(|light| !light.everywhere()) {
            self.lights.push(GpuLight::new(light));
            let range = light.range.expect("a light that's binned has a range");
            let reach = ahead.dot(light.position - viewpoint.position) + range;
            self.reach = self.reach.max(reach);
        }
        assert!(
            u32::try_from(self.lights.len()).is_ok(),
            "fewer than 2^32 lights"
        );
    }

    /// The lights, as shaders read them.
    pub(crate) fn lights(&self) -> &[GpuLight] {
        &self.lights
    }

    /// Declares in `graph` the clusters' buffer and the pass that copies the lights to its front
    /// and bins them into it, if a light is binned.
    ///
    /// # Panics
    ///
    /// If the lights' copies take 2^32 words or more.
    pub(crate) fn plan(&self, graph: &mut Graph) -> Option<ClusterPlan> {
        let binned = self.lights.len() as u32 - self.everywhere;
        if binned == 0 {
            return None;
        }
        let words = binned.div_ceil(32);
        let bits = size_of_val(self.lights.as_slice()) / 4;
        let bits = u32::try_from(bits).expect("the lights' copies take fewer than 2^32 words");
        let bytes = (u64::from(bits) + u64::from(CLUSTERS) * (2 + u64::from(words))) * 4;
        let buffer = graph.buffer("light clusters", bytes);
        let pass = (graph.compute(label!("light clusters")))
            .write_buffer(buffer)
            .add();
        Some(ClusterPlan {
            buffer,
            pass,
            words,
            bits,
        })
    }

    /// Records `plan`'s pass, the next of the frame's, which copies the lights, where the frame
    /// uploaded them, at `lights`, to the front of the clusters' buffer, and bins them into the
    /// clusters of what `viewpoint` sees of a view `aspect` times as wide as it's tall; and returns
    /// where the view's shaders read the lights, the copies, and what they read of the clusters:
    /// `lights` and no clusters if there's no plan, as no light is binned.
    pub(crate) fn bin(
        &self,
        frame: &mut Frame<'_>,
        plan: Option<&ClusterPlan>,
        lights: u64,
        viewpoint: &Viewpoint,
        aspect: f32,
    ) -> (u64, ClusterData) {
        let Some(plan) = plan else {
            let data = ClusterData {
                everywhere: self.everywhere,
                ..ClusterData::default()
            };
            return (lights, data);
        };
        let mut pass = frame.compute(plan.pass);
        let copies = pass.address(plan.buffer);
        let data = ClusterData {
            words: plan.words,
            bits: plan.bits,
            ..clusters(self.everywhere, self.reach, viewpoint, aspect)
        };
        let projection = viewpoint.projection_matrix(aspect);
        let view =
            Mat4::from_rotation_translation(viewpoint.rotation, viewpoint.position).inverse();
        let push = BinPush {
            right: view.row(0),
            up: view.row(1),
            ahead: data.depth,
            lights,
            copies,
            clusters: copies + u64::from(plan.bits) * 4,
            first: self.everywhere,
            count: self.lights.len() as u32 - self.everywhere,
            words: plan.words,
            across: ACROSS,
            down: DOWN,
            slices: SLICES,
            // Both projections are symmetric, so that the view's space's X and Y are clip space's
            // over these, times the depth with perspective.
            x_scale: 1.0 / projection.x_axis.x,
            y_scale: 1.0 / projection.y_axis.y,
            slice_scale: data.slice_scale,
            slice_bias: data.slice_bias,
            perspective: data.perspective,
            light_words: plan.bits,
        };
        pass.set_pipeline(&self.binning);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch([CLUSTERS, 1, 1]);
        (copies, data)
    }
}

/// The clusters of what `viewpoint` sees of a view `aspect` times as wide as it's tall, the first
/// `everywhere` of the frame's lights lighting everywhere, without their words: where a point is
/// among the tiles, by the view's projection, and its slice, exponentially with perspective, from
/// the near plane to `reach` ahead, as far as the lights binned reach, or twice the near plane's
/// distance if that's farther, and evenly without, from the near plane to the far.
fn clusters(everywhere: u32, reach: f32, viewpoint: &Viewpoint, aspect: f32) -> ClusterData {
    let clip = viewpoint.view_projection(aspect);
    let [x, y, _, w] = [0, 1, 2, 3].map(|row| clip.row(row));
    let ahead = viewpoint.rotation * Vec3::NEG_Z;
    let slices = SLICES as f32;
    let (slice_scale, slice_bias, perspective) = match viewpoint.projection {
        Projection::Perspective { near, .. } => {
            let far = reach.max(2.0 * near);
            let scale = slices / libm::log2f(far / near);
            (scale, -libm::log2f(near) * scale, 1)
        }
        Projection::Orthographic { near, far, .. } => {
            let (near, far) = (near.min(far), near.max(far));
            let scale = slices / (far - near);
            (scale, -near * scale, 0)
        }
    };
    ClusterData {
        everywhere,
        words: 0,
        bits: 0,
        // From clip space's -1 to 1 onto 0 to the tiles across, and down, as its Y points down.
        x: (x + w) * (0.5 * ACROSS as f32),
        y: (y + w) * (0.5 * DOWN as f32),
        w,
        depth: ahead.extend(-ahead.dot(viewpoint.position)),
        slice_scale,
        slice_bias,
        perspective,
        across: ACROSS,
        down: DOWN,
        slices: SLICES,
        unused: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use gg_core::math::Quat;

    use super::*;

    /// Where `data` puts `p`, as `cluster` in `shaders/model.slang` finds it: across and down the
    /// tiles, and its slice.
    fn placed(data: &ClusterData, p: Vec3) -> [f32; 3] {
        let q = p.extend(1.0);
        let w = data.w.dot(q);
        let depth = data.depth.dot(q);
        let along = if data.perspective == 1 {
            libm::log2f(depth)
        } else {
            depth
        };
        [
            data.x.dot(q) / w,
            data.y.dot(q) / w,
            along * data.slice_scale + data.slice_bias,
        ]
    }

    /// Whether `a` and `b` are within a thousandth of each other.
    fn near_to(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
    }

    /// The clusters span the view: its top left corner at the first tile, its bottom right past the
    /// last; with perspective, its near plane at the first slice and as far as the lights reach
    /// past the last, or twice the near plane's distance if they reach no farther, its slices the
    /// same number to each doubling of the distance; and without, from its near plane to its far,
    /// the same depth each.
    #[test]
    fn clusters_span_the_view() {
        let aspect = 16.0 / 9.0;
        let viewpoint = Viewpoint {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.5),
            projection: Projection::Perspective {
                fov: FRAC_PI_2,
                near: 0.5,
            },
        };
        let ahead = viewpoint.rotation * Vec3::NEG_Z;
        let right = viewpoint.rotation * Vec3::X;
        let up = viewpoint.rotation * Vec3::Y;
        // A quarter turn from bottom to top: at a distance ahead, the view reaches as far up, and
        // `aspect` times as far across.
        let seen = |depth: f32, across: f32, upward: f32| {
            let off = right * (across * depth * aspect) + up * (upward * depth);
            viewpoint.position + ahead * depth + off
        };
        let data = clusters(3, 32.0, &viewpoint, aspect);
        assert_eq!(data.everywhere, 3);
        assert!(near_to(
            placed(&data, seen(0.5, -1.0, 1.0)),
            [0.0, 0.0, 0.0]
        ));
        assert!(near_to(
            placed(&data, seen(32.0, 1.0, -1.0)),
            [16.0, 9.0, 24.0]
        ));
        // Six doublings from 0.5 to 32, four slices each: twelve slices in, three doublings.
        assert!(near_to(
            placed(&data, seen(4.0, 0.0, 0.0)),
            [8.0, 4.5, 12.0]
        ));
        let short = clusters(0, 0.6, &viewpoint, aspect);
        assert!(near_to(
            placed(&short, seen(1.0, 0.0, 0.0)),
            [8.0, 4.5, 24.0]
        ));
        let orthographic = Viewpoint {
            projection: Projection::Orthographic {
                height: 4.0,
                near: -2.0,
                far: 10.0,
            },
            ..viewpoint
        };
        let seen = |depth: f32, across: f32, upward: f32| {
            let off = right * (across * 2.0 * aspect) + up * (upward * 2.0);
            viewpoint.position + ahead * depth + off
        };
        let data = clusters(0, 32.0, &orthographic, aspect);
        assert!(near_to(
            placed(&data, seen(-2.0, -1.0, 1.0)),
            [0.0, 0.0, 0.0]
        ));
        assert!(near_to(
            placed(&data, seen(10.0, 1.0, -1.0)),
            [16.0, 9.0, 24.0]
        ));
        assert!(near_to(
            placed(&data, seen(4.0, 0.0, 0.0)),
            [8.0, 4.5, 12.0]
        ));
    }
}
