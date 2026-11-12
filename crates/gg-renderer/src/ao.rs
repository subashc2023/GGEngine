//! Ambient occlusion: how much of the light from all around reaches each point the game's view
//! shows, past what's near it, from the view's depth, as `shaders/ao.slang` computes it, XeGTAO's
//! ground-truth ambient occlusion, in compute passes once what's opaque in the view has drawn its
//! depth and before it's shaded, which darkens the sky's light by it, into an image of the view's
//! size; averaged over the frames before by a [`History`] of its own, so that its noise, which is on
//! the screen, doesn't stay there as the view moves.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2, Vec3};
use gg_core::{Result, label};
use gg_gpu::{
    BufferId, ComputePipeline, Device, Frame, Graph, ImageId, PassId, StorageImage, Target,
};

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

/// How many times the occlusion is denoised: three times, XeGTAO's soft preset, however the view is
/// antialiased. XeGTAO denoises once, its sharp preset, where temporal antialiasing averages its
/// noise over the frames; here the history averages 8 frames' noise in a still view, as
/// [`History::noise`] says, and 8 frames' denoised once averaged to a grainier occlusion than
/// denoised three times, by about a sixth, in the measure of the GPU test
/// `ambient_occlusion_averages_its_noise_away`, as temporal antialiasing averaged them; and what
/// bounds the history, what the last denoise's terms range over round each pixel, is the wider the
/// fewer denoises come before it, which leaves more of a trail behind what moves.
const DENOISES: usize = 3;

/// How many frames' noise in turn a still view's occlusion takes, which its history averages, as
/// many as temporal antialiasing has offsets, so that each offset draws a still pixel as it did a
/// cycle before, as the resolve asks of what it keeps unclipped; and how many such cycles of
/// XeGTAO's 64 frames' noise there are, which a view that moves goes on through.
const PHASES: u64 = 8;
const CYCLES: u64 = 8;

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
    accumulation: u64,
}

/// What the last denoise accumulates by: `Accumulation` in `shaders/ao.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Accumulation {
    previous_clip_from_view: Mat4,
    history: u32,
    accumulated: u32,
    taken: u32,
    unused: u32,
}

/// From the occlusion's view space, X right, Y up and Z ahead, as `shaders/ao.slang` has it, to the
/// clip space `previous_clip_from_world` takes the world to, through `world_from_view`, the view's,
/// whose space looks down -Z: where a frame before saw each point this frame's occlusion is of.
fn reprojection(previous_clip_from_world: Mat4, world_from_view: Mat4) -> Mat4 {
    previous_clip_from_world * world_from_view * Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0))
}

/// The ambient occlusion's history, which lasts from frame to frame: two images of the view's size,
/// made again as it changes, and dropped by a frame that renders the view without ambient
/// occlusion, each frame accumulating into the one the frame before didn't, against the other; how
/// the last frame that accumulated drew the view, which their remaking or dropping forgets; and which
/// frames' noise the frames take. Apart from [`AoPass`], as a frame takes its images as targets
/// while it records the passes.
#[derive(Default)]
pub(crate) struct History {
    images: Option<[StorageImage; 2]>,
    last: Option<Accumulated>,
    /// How many frames have accumulated, by which each takes the next of [`PHASES`] frames' noise;
    /// and how many of them saw from elsewhere than the frame before, by which the cycle of those
    /// that the frames take moves on: so that what's left of the noise as the view moves, which
    /// the history carries with the world, isn't the same few frames' over and over, which would
    /// stay on the screen as the view moved slowly. A view that moves every frame takes XeGTAO's 64
    /// in turn, as it orders them: the noise of each pixel, by a frame's place, is a step of a
    /// lattice on, which steps of 9 would take under a quarter as far along one of its two ways.
    accumulated: u64,
    moving: u64,
}

/// The last frame that accumulated: its number, the view's size, and the matrices from the world to
/// the view's clip space it drew by, with antialiasing's offset, which its history is of, and
/// without.
#[derive(Clone, Copy)]
struct Accumulated {
    frame: u64,
    size: [u32; 2],
    clip_from_world: Mat4,
    unjittered: Mat4,
}

impl History {
    /// Makes its images at `size`, if they're another size, or there are none, forgetting what
    /// those there accumulated.
    pub(crate) fn prepare(&mut self, device: &Arc<Device>, size: [u32; 2]) -> Result<()> {
        if (self.images.as_ref()).is_some_and(|images| images[0].size() == size) {
            return Ok(());
        }
        self.forget();
        let image = |name| StorageImage::new(device, size, 1, false, name);
        self.images = Some([
            image("ambient occlusion history 0")?,
            image("ambient occlusion history 1")?,
        ]);
        Ok(())
    }

    /// Lets its images go, as a frame that renders the view without ambient occlusion does, and what
    /// they accumulated: those made again hold nothing until a frame accumulates.
    pub(crate) fn forget(&mut self) {
        // They retire until the GPU is done with them.
        self.images = None;
        self.last = None;
    }

    /// The matrix from the world to the view's clip space, with antialiasing's offset, that what
    /// frame `frame` accumulates against was drawn by: the last frame's to accumulate, if it was the
    /// frame before, of a view of `size` pixels, and the view isn't `cut` to where it sees from.
    pub(crate) fn against(&self, frame: u64, size: [u32; 2], cut: bool) -> Option<Mat4> {
        (self.last)
            .filter(|last| !cut && last.frame + 1 == frame && last.size == size)
            .map(|last| last.clip_from_world)
    }

    /// Whether a frame that sees from `unjittered`, from the world to the view's clip space without
    /// antialiasing's offset, sees from elsewhere than the last frame to accumulate.
    fn moved(&self, unjittered: Mat4) -> bool {
        self.last.is_none_or(|last| last.unjittered != unjittered)
    }

    /// Which of XeGTAO's 64 frames' noise the next frame to accumulate takes, as it sees from
    /// `unjittered`: in a still view, each of [`PHASES`] in turn, as many as temporal antialiasing
    /// has offsets, whose cycle repeats; in one that moves, the cycle moves on every [`PHASES`]
    /// frames.
    pub(crate) fn noise(&self, unjittered: Mat4) -> u32 {
        let moving = self.moving + u64::from(self.moved(unjittered));
        let cycle = moving / PHASES % CYCLES;
        (self.accumulated % PHASES + PHASES * cycle) as u32
    }

    /// Notes that frame `frame`, submitted, accumulated the occlusion of a view of `size` pixels,
    /// drawn by `clip_from_world`, with antialiasing's offset, and `unjittered`, without it, for the
    /// next frame to accumulate against.
    pub(crate) fn accumulated(
        &mut self,
        frame: u64,
        size: [u32; 2],
        clip_from_world: Mat4,
        unjittered: Mat4,
    ) {
        self.moving += u64::from(self.moved(unjittered));
        self.accumulated += 1;
        self.last = Some(Accumulated {
            frame,
            size,
            clip_from_world,
            unjittered,
        });
    }

    /// Its images, as a frame that accumulates takes them as targets, at the indices
    /// [`AoPass::plan`] gives them: none before [`prepare`](Self::prepare) has made them.
    pub(crate) fn targets(&mut self) -> impl Iterator<Item = Target<'_>> {
        (self.images.iter_mut()).flat_map(|images| images.iter_mut().map(Target::Storage))
    }
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
/// nearer of, the buffers between its passes, the image it ends in, which the passes that shade the
/// view sample, the [`History`]'s image it accumulates into, and the one it accumulates against, if
/// the frame before accumulated one it can take, with the matrix from the world to the view's clip
/// space that drew it.
pub(crate) struct Plan {
    size: [u32; 2],
    depths: [ImageId; 2],
    levels: BufferId,
    /// What the occlusion writes, and the denoises read and write by turns.
    terms: [BufferId; 2],
    image: ImageId,
    accumulated: ImageId,
    history: Option<(ImageId, Mat4)>,
    prefilter: PassId,
    occlusion: PassId,
    denoises: [PassId; DENOISES],
}

impl Plan {
    /// The view's ambient occlusion: a scalar a pixel, the fraction of the light from all around
    /// that reaches it.
    pub(crate) fn image(&self) -> ImageId {
        self.image
    }
}

/// What a frame's ambient occlusion takes of how it draws the view: from the view's space to its
/// clip space, with antialiasing's offset, under perspective; from the view's space to the world;
/// the settings; and which of XeGTAO's 64 frames' noise it takes, as [`History::noise`] says.
pub(crate) struct Seen {
    pub(crate) clip_from_view: Mat4,
    pub(crate) world_from_view: Mat4,
    pub(crate) settings: AmbientOcclusion,
    pub(crate) noise: u32,
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

    /// Declares in `graph` the ambient occlusion of frame `frame`'s view, of `size` pixels, from
    /// `depths`, as the passes before drew them, whose nearer at each pixel is what's opaque there,
    /// into an image its shading passes then sample, denoised [`DENOISES`] times, the last of which
    /// accumulates it into the image of a [`History`]'s two, at targets `first` and 1 past it, that
    /// the frame before didn't accumulate into, against the other, if `against`, the matrix from the
    /// world to the view's clip space that drew it, is given, as [`History::against`] gives it.
    pub(crate) fn plan(
        graph: &mut Graph,
        depths: [ImageId; 2],
        size: [u32; 2],
        first: usize,
        frame: u64,
        against: Option<Mat4>,
    ) -> Plan {
        let (_, bytes) = levels(size);
        let levels = graph.buffer("ambient occlusion view depths", bytes);
        let pixels = u64::from(size[0]) * u64::from(size[1]) * 4;
        let terms = [
            graph.buffer("ambient occlusion terms", pixels),
            graph.buffer("ambient occlusion terms denoised", pixels),
        ];
        let image = graph.scalar("ambient occlusion", size);
        let images = [graph.target(first), graph.target(first + 1)];
        let (into, from) = ((frame % 2) as usize, ((frame + 1) % 2) as usize);
        let accumulated = images[into];
        let history = against.map(|matrix| (images[from], matrix));
        let prefilter = graph.compute(label!("ambient occlusion prefilter"));
        let prefilter = if depths[0] == depths[1] {
            prefilter.sample(depths[0])
        } else {
            prefilter.sample(depths[0]).sample(depths[1])
        };
        let prefilter = prefilter.write_buffer(levels).add();
        let occlusion = (graph.compute(label!("ambient occlusion")))
            .read_buffer(levels)
            .write_buffer(terms[0])
            .add();
        let denoises = std::array::from_fn(|i| {
            let pass = graph
                .compute(label!("ambient occlusion denoise"))
                .read_buffer(terms[i % 2]);
            let pass = if i + 1 == DENOISES {
                let pass = pass.read_buffer(levels);
                let pass = match history {
                    Some((history, _)) => pass.sample(history),
                    None => pass,
                };
                pass.overwrite(image).overwrite(accumulated)
            } else {
                pass.write_buffer(terms[(i + 1) % 2])
            };
            pass.add()
        });
        Plan {
            size,
            depths,
            levels,
            terms,
            image,
            accumulated,
            history,
            prefilter,
            occlusion,
            denoises,
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
            noise: seen.noise,
            blur: BLUR,
            starts: levels(plan.size).0,
            unused: 0,
            accumulation: 0,
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
        let term = |i: usize| plan.terms[i % 2];
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
        for (i, &denoise) in plan.denoises.iter().enumerate() {
            let last = i + 1 == DENOISES;
            let mut pass = frame.compute(denoise);
            let push = if last {
                let (history, previous) = match plan.history {
                    Some((history, previous)) => (pass.sampled(history), Some(previous)),
                    None => (0, None),
                };
                let accumulation = Accumulation {
                    previous_clip_from_view: previous.map_or(Mat4::IDENTITY, |previous| {
                        reprojection(previous, seen.world_from_view)
                    }),
                    history,
                    accumulated: pass.storage(plan.accumulated, 0),
                    taken: u32::from(previous.is_some()),
                    unused: 0,
                };
                Push {
                    levels: pass.address(plan.levels),
                    terms: pass.address(term(i)),
                    target: pass.storage(plan.image, 0),
                    accumulation: pass.upload(bytemuck::bytes_of(&accumulation))?.address(),
                    ..base
                }
            } else {
                Push {
                    terms: pass.address(term(i)),
                    denoised: pass.address(term(i + 1)),
                    blur: BLUR / 5.0,
                    ..base
                }
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

    /// A point in the occlusion's view space, as a jittered projection's pixels unproject to it,
    /// reprojects to where a view that moved and turned since saw it, offset as it was: in the clip
    /// space that view's own matrix takes the point's place in the world to, its W the point's
    /// distance ahead of that view.
    #[test]
    fn points_reproject_to_where_the_frame_before_saw_them() {
        let view_from_world = |viewpoint: &Viewpoint| {
            Mat4::from_rotation_translation(viewpoint.rotation, viewpoint.position).inverse()
        };
        let now = Viewpoint {
            position: Vec3::new(3.0, 1.0, -2.0),
            rotation: Quat::from_rotation_y(0.4),
            projection: Projection::Perspective {
                fov: 1.1,
                near: 0.05,
            },
        };
        let before = Viewpoint {
            position: Vec3::new(2.6, 1.2, -1.5),
            rotation: Quat::from_rotation_y(0.32) * Quat::from_rotation_x(-0.05),
            ..now
        };
        let world_from_view = Mat4::from_rotation_translation(now.rotation, now.position);
        let previous = jitter(Vec2::new(-0.0008, 0.0011)) * before.view_projection(16.0 / 9.0);
        let reprojected = reprojection(previous, world_from_view);
        for world in [
            Vec3::new(3.0, 1.0, -6.0),
            Vec3::new(0.5, -0.3, -9.0),
            Vec3::new(-14.0, 6.0, -60.0),
        ] {
            // X right, Y up and Z ahead, as the occlusion's view space has it.
            let seen = view_from_world(&now).transform_point3(world);
            let point = Vec3::new(seen.x, seen.y, -seen.z);
            let clip = reprojected * point.extend(1.0);
            let expected = previous * world.extend(1.0);
            assert!(
                (clip - expected).abs().max_element() < 1.0e-3 * expected.w,
                "{clip} {expected}"
            );
            let ahead = -view_from_world(&before).transform_point3(world).z;
            assert!(
                (clip.w - ahead).abs() < 1.0e-4 * ahead,
                "{} {ahead}",
                clip.w
            );
        }
    }

    /// A still view takes 8 frames' noise in turn, over and over; one that moves goes on through
    /// all 64, a frame's on from the one before's but at the end of each 8; and one that stops
    /// again takes the 8 it stopped among.
    #[test]
    fn a_still_view_cycles_through_8_frames_noise_and_a_moving_one_through_64() {
        let mut history = History::default();
        let size = [64, 32];
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let mut taken = Vec::new();
        let mut take = |history: &mut History, frame: u64, seen: Mat4| {
            taken.push(history.noise(seen));
            history.accumulated(frame, size, seen, seen);
        };
        for frame in 1..=24 {
            take(&mut history, frame, at(0.0));
        }
        for frame in 25..=88 {
            take(&mut history, frame, at(frame as f32));
        }
        for frame in 89..=112 {
            take(&mut history, frame, at(88.0));
        }
        let still = &taken[..24];
        assert!((0..16).all(|i| still[i] == still[i + 8]), "{still:?}");
        let mut cycle = still[..8].to_vec();
        cycle.sort_unstable();
        cycle.dedup();
        assert_eq!(cycle.len(), 8, "{still:?}");
        let mut moving = taken[24..88].to_vec();
        moving.sort_unstable();
        assert_eq!(
            moving,
            (0..64).collect::<Vec<u32>>(),
            "{:?}",
            &taken[24..88]
        );
        let stopped = &taken[88..];
        assert!((0..16).all(|i| stopped[i] == stopped[i + 8]), "{stopped:?}");
        assert!(taken.iter().all(|&noise| noise < 64));
    }

    /// What a frame accumulates against is the frame before's, of the view at its size, and none
    /// once a frame was skipped, the view is another size, or it's cut to where it sees from.
    #[test]
    fn a_history_is_taken_only_from_the_frame_before() {
        let mut history = History::default();
        let drawn = Mat4::from_translation(Vec3::X);
        assert_eq!(history.against(1, [64, 32], false), None);
        history.accumulated(7, [64, 32], drawn, Mat4::IDENTITY);
        assert_eq!(history.against(8, [64, 32], false), Some(drawn));
        assert_eq!(history.against(9, [64, 32], false), None);
        assert_eq!(history.against(8, [64, 30], false), None);
        assert_eq!(history.against(8, [64, 32], true), None);
    }
}
