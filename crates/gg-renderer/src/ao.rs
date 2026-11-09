//! Ambient occlusion: how much of the light from all around reaches each point the game's view
//! shows, past what's near it, from the view's depth, as `shaders/ao.slang` computes it, XeGTAO's
//! ground-truth ambient occlusion, in compute passes once what's opaque in the view has drawn its
//! depth and before it's shaded, which darkens the sky's light by it, into an image of the view's
//! size.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2};
use gg_core::{Result, label};
use gg_gpu::{BufferId, ComputePipeline, Device, Frame, Graph, ImageId, PassId};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ao.spv"));

/// How many pixels across and down a workgroup of the occlusion or a denoise takes: `GROUP` in
/// `shaders/ao.slang`. The prefilter's threads take 2 by 2 pixels each.
const GROUP: u32 = 8;

/// How many levels the view-space depth has: `LEVELS` in `shaders/ao.slang`, XeGTAO's
/// `XE_GTAO_DEPTH_MIP_LEVELS`.
const LEVELS: u32 = 5;

/// The power a strength of 1 raises the visibility to: XeGTAO's `XE_GTAO_DEFAULT_FINAL_VALUE_POWER`.
const FINAL_VALUE_POWER: f32 = 2.2;

/// How much a denoise keeps of each pixel's own term, against its neighbors' weights: the last's,
/// XeGTAO's `DenoiseBlurBeta`, and those before it a fifth of that, which blur more.
const BLUR: f32 = 1.2;

/// How many times the occlusion is denoised: once, as XeGTAO's sharp preset, where the view is
/// antialiased, which takes noise that changes each frame and averages it over frames; three times,
/// its soft preset, where it isn't, and the noise stays still.
const ANTIALIASED_DENOISES: usize = 1;
const DENOISES: usize = 3;

/// How the game's view's ambient occlusion darkens the light from all around, as
/// [`RenderData::ambient_occlusion`](crate::RenderData::ambient_occlusion) has it: the sky's diffuse
/// and specular light at each point the view shows, by how much of it what's near the point hides,
/// as its depth shows it, and never the frame's lights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientOcclusion {
    /// How far from a point what hides its light reaches, in the world's units, fading over the
    /// farther part: 0.5 by default, XeGTAO's. A radius that isn't positive and finite occludes
    /// nothing, and none of it runs.
    pub radius: f32,
    /// How much it darkens: the power the light it lets through is raised to, as a fraction, times
    /// XeGTAO's 2.2, so that 1, the default, darkens as XeGTAO does, and more darkens more. A
    /// strength that isn't positive and finite darkens nothing, and none of it runs.
    pub strength: f32,
}

impl Default for AmbientOcclusion {
    fn default() -> Self {
        Self {
            radius: 0.5,
            strength: 1.0,
        }
    }
}

impl AmbientOcclusion {
    /// Whether it darkens anything: its radius and its strength are positive and finite.
    pub(crate) fn darkens(&self) -> bool {
        let positive = |value: f32| value.is_finite() && value > 0.0;
        positive(self.radius) && positive(self.strength)
    }
}

/// What takes the view's pixels back into its space, as `Push` in `shaders/ao.slang` has it: a
/// point's X and Y there over its depth, from its texture coordinates in the view, from its top
/// left, `(view_mul * uv + view_add) * depth`; and its view-space depth, its distance ahead of the
/// view, from the view's depth, `z`, `depth_mul / (depth_add + z)`; as the reflections' trace takes
/// it too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Unprojection {
    pub(crate) view_mul: Vec2,
    pub(crate) view_add: Vec2,
    pub(crate) depth_mul: f32,
    pub(crate) depth_add: f32,
}

impl Unprojection {
    /// From a perspective's `clip_from_view`, as [`Viewpoint`](crate::Viewpoint)'s projection
    /// makes it and antialiasing moves it: its fourth row (0, 0, -1, 0), each point's clip W its
    /// distance ahead of the view, `d`, its view space's -Z, so that its normalized X is
    /// `(row0.x * x - row0.z * d) / d`, and Y alike, and its depth `(row2.w - row2.z * d) / d`,
    /// which this solves for X, Y and `d`. A view's normalized coordinates are `uv * 2 - 1`, Y down
    /// as Vulkan's clip space has it, so that the Y found is the view space's, up.
    pub(crate) fn new(clip_from_view: Mat4) -> Self {
        let [row0, row1, row2] = [0, 1, 2].map(|row| clip_from_view.row(row));
        Self {
            view_mul: Vec2::new(2.0 / row0.x, 2.0 / row1.y),
            view_add: Vec2::new((row0.z - 1.0) / row0.x, (row1.z - 1.0) / row1.y),
            depth_mul: row2.w,
            depth_add: row2.z,
        }
    }
}

/// What every pass pushes: `Push` in `shaders/ao.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Push {
    levels: u64,
    terms: u64,
    denoised: u64,
    view_mul: [f32; 2],
    view_add: [f32; 2],
    depth_mul: f32,
    depth_add: f32,
    size: [u32; 2],
    depths: [u32; 2],
    target: u32,
    radius: f32,
    power: f32,
    noise: u32,
    blur: f32,
    starts: [u32; LEVELS as usize - 1],
    unused: u32,
}

/// The view-space depth's levels of a view of `size` pixels, one after another, each half the size
/// of the one before, rounded down, at least a texel, as the shader reads them: where each level
/// after the first starts, in texels, and their bytes together.
fn levels(size: [u32; 2]) -> ([u32; LEVELS as usize - 1], u64) {
    let [width, height] = size.map(u64::from);
    let texels = |level: u32| (width >> level).max(1) * (height >> level).max(1);
    let mut starts = [0; LEVELS as usize - 1];
    let mut start = texels(0);
    for (level, at) in (1..).zip(&mut starts) {
        // Within a texture's largest size squared, which a u32 holds.
        *at = start as u32;
        start += texels(level);
    }
    (starts, start * 4)
}

/// What computes the view's ambient occlusion: the prefilter of its depth, the occlusion, the
/// denoises before the last and the last.
pub(crate) struct AoPass {
    prefilter: ComputePipeline,
    occlusion: ComputePipeline,
    denoise: ComputePipeline,
    denoise_last: ComputePipeline,
}

/// A frame's ambient occlusion in its graph: its passes, the view's size, the depths it takes the
/// nearer of, the buffers between its passes, and the image it ends in, which the passes that
/// shade the view sample.
pub(crate) struct Plan {
    size: [u32; 2],
    depths: [ImageId; 2],
    levels: BufferId,
    /// What the occlusion writes, and the denoises read and write by turns, the second only where
    /// it's denoised more than once.
    terms: [Option<BufferId>; 2],
    image: ImageId,
    prefilter: PassId,
    occlusion: PassId,
    denoises: [Option<PassId>; DENOISES],
    antialiased: bool,
}

impl Plan {
    /// The view's ambient occlusion: a scalar a pixel, the fraction of the light from all around
    /// that reaches it.
    pub(crate) fn image(&self) -> ImageId {
        self.image
    }
}

/// What a frame's ambient occlusion takes of how it draws the view: from the view's space to its
/// clip space, with antialiasing's offset, under perspective; the settings; and the frame's number,
/// whose noise an antialiased view takes.
pub(crate) struct Seen {
    pub(crate) clip_from_view: Mat4,
    pub(crate) settings: AmbientOcclusion,
    pub(crate) frame: u64,
}

impl AoPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        Ok(Self {
            prefilter: pipeline("ambient occlusion prefilter", c"prefilter_main")?,
            occlusion: pipeline("ambient occlusion", c"occlusion_main")?,
            denoise: pipeline("ambient occlusion denoise", c"denoise_main")?,
            denoise_last: pipeline("ambient occlusion denoise last", c"denoise_last_main")?,
        })
    }

    /// Declares in `graph` the ambient occlusion of a view of `size` pixels, from `depths`, as the
    /// passes before drew them, whose nearer at each pixel is what's opaque there, into an image
    /// its shading passes then sample; denoised as [`ANTIALIASED_DENOISES`] says if `antialiased`,
    /// and as [`DENOISES`] does otherwise.
    pub(crate) fn plan(
        graph: &mut Graph,
        depths: [ImageId; 2],
        size: [u32; 2],
        antialiased: bool,
    ) -> Plan {
        let count = if antialiased {
            ANTIALIASED_DENOISES
        } else {
            DENOISES
        };
        let (_, bytes) = levels(size);
        let levels = graph.buffer("ambient occlusion view depths", bytes);
        let pixels = u64::from(size[0]) * u64::from(size[1]) * 4;
        let terms = [
            Some(graph.buffer("ambient occlusion terms", pixels)),
            (count > 1).then(|| graph.buffer("ambient occlusion terms denoised", pixels)),
        ];
        let image = graph.scalar("ambient occlusion", size);
        let prefilter = graph.compute(label!("ambient occlusion prefilter"));
        let prefilter = if depths[0] == depths[1] {
            prefilter.sample(depths[0])
        } else {
            prefilter.sample(depths[0]).sample(depths[1])
        };
        let prefilter = prefilter.write_buffer(levels).add();
        let term = |i: usize| terms[i % 2].expect("declared for as many denoises");
        let occlusion = (graph.compute(label!("ambient occlusion")))
            .read_buffer(levels)
            .write_buffer(term(0))
            .add();
        let mut denoises = [None; DENOISES];
        for (i, denoise) in denoises.iter_mut().take(count).enumerate() {
            let pass = graph
                .compute(label!("ambient occlusion denoise"))
                .read_buffer(term(i));
            let pass = if i + 1 == count {
                pass.overwrite(image)
            } else {
                pass.write_buffer(term(i + 1))
            };
            *denoise = Some(pass.add());
        }
        Plan {
            size,
            depths,
            levels,
            terms,
            image,
            prefilter,
            occlusion,
            denoises,
            antialiased,
        }
    }

    /// Records `plan`'s passes, the next of the frame's, as `seen` says the frame draws the view.
    pub(crate) fn record(&self, frame: &mut Frame<'_>, plan: &Plan, seen: &Seen) -> Result<()> {
        let Unprojection {
            view_mul,
            view_add,
            depth_mul,
            depth_add,
        } = Unprojection::new(seen.clip_from_view);
        let base = Push {
            levels: 0,
            terms: 0,
            denoised: 0,
            view_mul: view_mul.to_array(),
            view_add: view_add.to_array(),
            depth_mul,
            depth_add,
            size: plan.size,
            depths: [0; 2],
            target: 0,
            radius: seen.settings.radius,
            power: FINAL_VALUE_POWER * seen.settings.strength,
            // Still without antialiasing, which would leave the noise to shimmer.
            noise: if plan.antialiased {
                (seen.frame % 64) as u32
            } else {
                0
            },
            blur: BLUR,
            starts: levels(plan.size).0,
            unused: 0,
        };
        let [across, down] = plan.size.map(|side| side.div_ceil(GROUP));
        let mut pass = frame.compute(plan.prefilter);
        let push = Push {
            levels: pass.address(plan.levels),
            depths: plan.depths.map(|depth| pass.sampled(depth)),
            ..base
        };
        pass.set_pipeline(&self.prefilter);
        pass.push_constants(bytemuck::bytes_of(&push));
        // Each thread takes 2 by 2 pixels.
        let [prefilter_across, prefilter_down] = plan.size.map(|side| side.div_ceil(2 * GROUP));
        pass.dispatch([prefilter_across, prefilter_down, 1]);
        drop(pass);
        let term = |i: usize| plan.terms[i % 2].expect("declared for as many denoises");
        let mut pass = frame.compute(plan.occlusion);
        let push = Push {
            levels: pass.address(plan.levels),
            terms: pass.address(term(0)),
            ..base
        };
        pass.set_pipeline(&self.occlusion);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch([across, down, 1]);
        drop(pass);
        let count = plan.denoises.iter().flatten().count();
        for (i, &denoise) in plan.denoises.iter().flatten().enumerate() {
            let last = i + 1 == count;
            let mut pass = frame.compute(denoise);
            let push = Push {
                terms: pass.address(term(i)),
                denoised: if last { 0 } else { pass.address(term(i + 1)) },
                target: if last { pass.storage(plan.image, 0) } else { 0 },
                blur: if last { BLUR } else { BLUR / 5.0 },
                ..base
            };
            pass.set_pipeline(if last {
                &self.denoise_last
            } else {
                &self.denoise
            });
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch([across, down, 1]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{Quat, Vec3};

    use super::*;
    use crate::taa::jitter;
    use crate::{Projection, Viewpoint};

    /// A point in the view's space, projected by a perspective that antialiasing moves a fraction
    /// of a pixel, comes back from its texture coordinates and depth where it was: X right, Y up
    /// and its distance ahead, which the view space's -Z is.
    #[test]
    fn pixels_unproject_to_where_their_points_are() {
        let viewpoint = Viewpoint {
            position: Vec3::new(3.0, 1.0, -2.0),
            rotation: Quat::from_rotation_y(0.4),
            projection: Projection::Perspective {
                fov: 1.1,
                near: 0.05,
            },
        };
        let aspect = 16.0 / 9.0;
        let offset = Vec2::new(0.0013, -0.0021);
        let clip_from_view = jitter(offset) * viewpoint.projection_matrix(aspect);
        let unprojected = Unprojection::new(clip_from_view);
        for point in [
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(1.5, -0.7, -4.0),
            Vec3::new(-20.0, 9.0, -60.0),
        ] {
            let clip = clip_from_view * point.extend(1.0);
            let ndc = clip / clip.w;
            let uv = (Vec2::new(ndc.x, ndc.y) + 1.0) / 2.0;
            let depth = unprojected.depth_mul / (unprojected.depth_add + ndc.z);
            let ahead = -point.z;
            assert!(
                (depth - ahead).abs() < ahead * 1.0e-5,
                "{depth} for {ahead}"
            );
            let xy = (unprojected.view_mul * uv + unprojected.view_add) * depth;
            let found = Vec3::new(xy.x, xy.y, depth);
            let expected = Vec3::new(point.x, point.y, ahead);
            assert!(
                (found - expected).abs().max_element() < 1.0e-4 * ahead,
                "{found} {expected}"
            );
        }
        // The view's top row, where V is 0, is up its space; and its depth of 0 is infinitely far.
        assert!(unprojected.view_add.y > 0.0, "{}", unprojected.view_add.y);
        assert_eq!(
            unprojected.depth_mul / (unprojected.depth_add + 0.0),
            f32::INFINITY
        );
    }

    /// The levels each halve the view, rounded down, to a texel at least, one after another, as
    /// the shader reads them.
    #[test]
    fn the_depths_levels_halve_the_view() {
        let sizes = [1920 * 1080, 960 * 540, 480 * 270, 240 * 135, 120 * 67];
        let starts = [
            sizes[0],
            sizes[0] + sizes[1],
            sizes[0] + sizes[1] + sizes[2],
        ];
        let starts = [starts[0], starts[1], starts[2], starts[2] + sizes[3]];
        let texels: u32 = sizes.iter().sum();
        assert_eq!(levels([1920, 1080]), (starts, u64::from(texels) * 4));
        assert_eq!(levels([3, 1]), ([3, 4, 5, 6], (3 + 1 + 1 + 1 + 1) * 4));
    }

    /// Settings that would occlude nothing, or occlude as no number, run none of it.
    #[test]
    fn only_a_positive_finite_radius_and_strength_darken() {
        assert!(AmbientOcclusion::default().darkens());
        for (radius, strength) in [
            (0.0, 1.0),
            (-1.0, 1.0),
            (f32::NAN, 1.0),
            (f32::INFINITY, 1.0),
            (0.5, 0.0),
            (0.5, f32::NAN),
        ] {
            assert!(!AmbientOcclusion { radius, strength }.darkens());
        }
    }
}
