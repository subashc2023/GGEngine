//! Ambient occlusion: how much of the light from all around reaches each point the view shows, past
//! what's near it, from the view's depth. `shaders/ao.slang` computes XeGTAO's ground-truth ambient
//! occlusion in compute passes, after the opaque depth and before shading, into an image of the
//! view's size. A [`History`] averages it over frames, so its screen-space noise doesn't stay put
//! as the view moves.
//!
//! A view of more than [`HALF_ABOVE`] pixels computes it at half its size across and down, as
//! [`occlusion_size`] says. A last pass then upsamples it to the view's size, weighting each
//! neighbor by how near its depth is.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2, Vec3};
use gg_core::{Result, label};
use gg_gpu::{
    BufferId, ComputePipeline, Device, Frame, Graph, ImageId, PassId, StorageImage, Target,
};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ao.spv"));

/// Pixels across and down per occlusion or denoise workgroup: `GROUP` in `shaders/ao.slang`. Each
/// prefilter thread takes 2 by 2 pixels.
const GROUP: u32 = 8;

/// The view-space depth's levels: `LEVELS` in `shaders/ao.slang`, XeGTAO's
/// `XE_GTAO_DEPTH_MIP_LEVELS`.
const LEVELS: u32 = 5;

/// The power a strength of 1 raises visibility to: XeGTAO's `XE_GTAO_DEFAULT_FINAL_VALUE_POWER`.
const FINAL_VALUE_POWER: f32 = 2.2;

/// How much a denoise keeps of each pixel's own term against its neighbors': XeGTAO's
/// `DenoiseBlurBeta` for the last, a fifth of it for those before, which blur more.
const BLUR: f32 = 1.2;

/// Denoises per frame: XeGTAO's soft preset, whatever the antialiasing. XeGTAO's sharp preset (one)
/// relies on temporal antialiasing; here the history averages 8 frames, as [`History::noise`] says.
/// One denoise averaged that way is grainier than three by about a sixth, per the GPU test
/// `ambient_occlusion_averages_its_noise_away`. Fewer denoises also widen the range that bounds the
/// history, leaving more trail behind what moves.
const DENOISES: usize = 3;

/// Frames of noise a still view cycles through, which its history averages: as many as temporal
/// antialiasing's offsets, so each offset draws a still pixel as it did a cycle before, as the
/// resolve asks of what it keeps unclipped. Also the number of such cycles in XeGTAO's 64 frames of
/// noise, which a moving view goes on through.
const PHASES: u64 = 8;
const CYCLES: u64 = 8;

/// The most pixels a view has for its occlusion to be computed at its own size: 1920 by 1080.
/// Past it, as on a Retina screen, its cost grows with the pixels while a pixel's detail shows
/// less. At 3024 by 1890 on an M3 Pro, the full size took 6.2 ms and half 2. Half loses the
/// darkening of creases a few pixels across, which the shaded view there hardly shows.
const HALF_ABOVE: u64 = 1920 * 1080;

/// The size a view of `size` computes its ambient occlusion at: half, rounded up, if `half` says
/// so or, unasked, if the view has more than [`HALF_ABOVE`] pixels; else its own.
pub(crate) fn occlusion_size(size: [u32; 2], half: Option<bool>) -> [u32; 2] {
    let pixels = u64::from(size[0]) * u64::from(size[1]);
    if half.unwrap_or(pixels > HALF_ABOVE) {
        size.map(|side| side.div_ceil(2))
    } else {
        size
    }
}

/// How the view's ambient occlusion darkens the light from all around, as
/// [`RenderData::ambient_occlusion`](crate::RenderData::ambient_occlusion) has it.
///
/// It darkens the sky's diffuse and specular light by how much of it nearby depth hides, never the
/// frame's lights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientOcclusion {
    /// How far what hides a point's light reaches, in world units, fading over the farther part.
    /// 0.5 by default, XeGTAO's. Not positive and finite: nothing occludes, and none of it runs.
    pub radius: f32,
    /// How much it darkens: the light let through is raised to this times XeGTAO's 2.2. 1 (the
    /// default) darkens as XeGTAO does; more darkens more. Not positive and finite: nothing
    /// darkens, and none of it runs.
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
    /// Whether radius and strength are both positive and finite.
    pub(crate) fn darkens(&self) -> bool {
        let positive = |value: f32| value.is_finite() && value > 0.0;
        positive(self.radius) && positive(self.strength)
    }
}

/// Unprojects the view's pixels, as `Push` in `shaders/ao.slang` has it, and as the reflections'
/// trace does too. From texture coordinates (top left origin), a point's view-space X and Y are
/// `(view_mul * uv + view_add) * depth`; from the view's depth `z`, its view-space depth (distance
/// ahead) is `depth_mul / (depth_add + z)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Unprojection {
    pub(crate) view_mul: Vec2,
    pub(crate) view_add: Vec2,
    pub(crate) depth_mul: f32,
    pub(crate) depth_add: f32,
}

impl Unprojection {
    /// From a perspective's `clip_from_view`, as [`Viewpoint`](crate::Viewpoint)'s projection makes
    /// it and antialiasing moves it. Its fourth row is (0, 0, -1, 0), so clip W is the distance
    /// ahead, `d`, the view space's -Z. Normalized X is `(row0.x * x - row0.z * d) / d`, Y alike,
    /// and depth `(row2.w - row2.z * d) / d`, which this solves for X, Y and `d`. Normalized
    /// coordinates are `uv * 2 - 1`, Y down as in Vulkan's clip space, so the Y found points up.
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

/// `Push` in `shaders/ao.slang`, field for field, for every pass.
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
    view: [u32; 2],
    depths: [u32; 2],
    target: u32,
    radius: f32,
    power: f32,
    noise: u32,
    blur: f32,
    starts: [u32; LEVELS as usize - 1],
    step: u32,
    occluded: u32,
    unused: u32,
    accumulation: u64,
}

/// `Accumulation` in `shaders/ao.slang`, field for field, for the last denoise.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Accumulation {
    previous_clip_from_view: Mat4,
    history: u32,
    accumulated: u32,
    taken: u32,
    unused: u32,
}

/// From the occlusion's view space (X right, Y up, Z ahead) to `previous_clip_from_world`'s clip
/// space, through `world_from_view` (looking down -Z): where an earlier frame saw each point.
fn reprojection(previous_clip_from_world: Mat4, world_from_view: Mat4) -> Mat4 {
    previous_clip_from_world * world_from_view * Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0))
}

/// The ambient occlusion's history, kept from frame to frame. Two images of the occlusion's size
/// ([`occlusion_size`]), remade when it changes and dropped by a frame without ambient occlusion; each frame accumulates into
/// the one the last didn't, against the other. It also keeps how the last accumulating frame drew
/// the view (forgotten with the images), and which frames' noise frames take. It's apart from
/// [`AoPass`] because a frame takes its images as targets while recording.
#[derive(Default)]
pub(crate) struct History {
    images: Option<[StorageImage; 2]>,
    last: Option<Accumulated>,
    /// Frames accumulated, which pick each frame's noise among [`PHASES`]. `moving` counts those
    /// that saw from elsewhere than the frame before, which moves the cycle on. So the noise left
    /// in a moving view, which the history carries with the world, isn't the same few frames' over
    /// and over, which would stay on screen in a slow move. A view moving every frame takes
    /// XeGTAO's 64 in its order: each pixel's noise steps a lattice by a frame's place, which steps
    /// of 9 would cover under a quarter as far along one of its two ways.
    accumulated: u64,
    moving: u64,
}

/// The last frame that accumulated: its number, the view's size (not the occlusion's), and its
/// world-to-clip matrices
/// with antialiasing's offset (the history's) and without.
#[derive(Clone, Copy)]
struct Accumulated {
    frame: u64,
    size: [u32; 2],
    clip_from_world: Mat4,
    unjittered: Mat4,
}

impl History {
    /// Moves what it keeps into the space of a frame whose origin is `by` further along: what was
    /// at `p` is at `p - by`, as [`RenderData::origin`](crate::RenderData::origin) says.
    pub(crate) fn moved(&mut self, by: Vec3) {
        if let Some(last) = &mut self.last {
            let along = Mat4::from_translation(by);
            last.clip_from_world *= along;
            last.unjittered *= along;
        }
    }

    /// Makes its images at `size` if there are none or they're another size, forgetting what was
    /// accumulated.
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

    /// Drops its images and what they accumulated, as a frame without ambient occlusion does.
    /// Images made again hold nothing until a frame accumulates.
    pub(crate) fn forget(&mut self) {
        // They retire once the GPU is done with them.
        self.images = None;
        self.last = None;
    }

    /// The world-to-clip matrix, with antialiasing's offset, that frame `frame` accumulates
    /// against: the last accumulating frame's, if it was the frame before, at `size`, and the view
    /// isn't `cut`.
    pub(crate) fn against(&self, frame: u64, size: [u32; 2], cut: bool) -> Option<Mat4> {
        (self.last)
            .filter(|last| !cut && last.frame + 1 == frame && last.size == size)
            .map(|last| last.clip_from_world)
    }

    /// Whether `unjittered` (world to clip, without antialiasing's offset) sees from elsewhere than
    /// the last accumulating frame.
    fn sees_elsewhere(&self, unjittered: Mat4) -> bool {
        self.last.is_none_or(|last| last.unjittered != unjittered)
    }

    /// Which of XeGTAO's 64 frames of noise the next accumulating frame takes. A still view cycles
    /// through [`PHASES`] of them; a moving one moves the cycle on every [`PHASES`] frames.
    pub(crate) fn noise(&self, unjittered: Mat4) -> u32 {
        let moving = self.moving + u64::from(self.sees_elsewhere(unjittered));
        let cycle = moving / PHASES % CYCLES;
        (self.accumulated % PHASES + PHASES * cycle) as u32
    }

    /// Notes that submitted frame `frame` accumulated at `size`, drawn by `clip_from_world` (with
    /// antialiasing's offset) and `unjittered` (without), for the next frame to accumulate against.
    pub(crate) fn accumulated(
        &mut self,
        frame: u64,
        size: [u32; 2],
        clip_from_world: Mat4,
        unjittered: Mat4,
    ) {
        self.moving += u64::from(self.sees_elsewhere(unjittered));
        self.accumulated += 1;
        self.last = Some(Accumulated {
            frame,
            size,
            clip_from_world,
            unjittered,
        });
    }

    /// Its images as targets, at the indices [`AoPass::plan`] gives them. None before
    /// [`prepare`](Self::prepare).
    pub(crate) fn targets(&mut self) -> impl Iterator<Item = Target<'_>> {
        (self.images.iter_mut()).flat_map(|images| images.iter_mut().map(Target::Storage))
    }
}

/// The view-space depth's levels at `size`, back to back, each half the last, rounded down, at
/// least a texel, as the shader reads them. Returns where each level after the first starts, in
/// texels, and their total bytes.
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

/// Computes the view's ambient occlusion: depth prefilter, occlusion, the denoises before the last,
/// the last, and at half size the upsample.
pub(crate) struct AoPass {
    prefilter: ComputePipeline,
    occlusion: ComputePipeline,
    denoise: ComputePipeline,
    denoise_last: ComputePipeline,
    upsample: ComputePipeline,
}

/// A frame's ambient occlusion in its graph. It takes the nearer of `depths`, and `history` is the
/// image it accumulates against, with the matrix that drew it.
pub(crate) struct Plan {
    /// The occlusion's size, and the view's.
    size: [u32; 2],
    view: [u32; 2],
    depths: [ImageId; 2],
    levels: BufferId,
    /// The occlusion writes them, and the denoises read and write them by turns.
    terms: [BufferId; 2],
    image: ImageId,
    accumulated: ImageId,
    history: Option<(ImageId, Mat4)>,
    prefilter: PassId,
    occlusion: PassId,
    denoises: [PassId; DENOISES],
    /// At half size alone: from the last denoise's accumulated image into `image`.
    upsample: Option<PassId>,
}

impl Plan {
    /// View pixels across and down per pixel of the occlusion: 1 or 2.
    fn step(&self) -> u32 {
        if self.size == self.view { 1 } else { 2 }
    }

    /// The view's ambient occlusion: per pixel, the fraction of the light from all around that
    /// reaches it.
    pub(crate) fn image(&self) -> ImageId {
        self.image
    }
}

/// How a frame draws the view, for its ambient occlusion: `clip_from_view` is a perspective, with
/// antialiasing's offset, and `noise` is [`History::noise`]'s.
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
            upsample: pipeline("ambient occlusion upsample", c"upsample_main")?,
        })
    }

    /// Declares in `graph` frame `frame`'s ambient occlusion at `size` ([`occlusion_size`] of
    /// `view`, the view's size), from `depths` (the nearest is what's opaque), into an image of the
    /// view's size shading then samples. It's denoised [`DENOISES`] times. The last denoise
    /// accumulates into whichever of a [`History`]'s two images (targets `first` and `first + 1`)
    /// the frame before didn't, against the other, if `against` gives that frame's matrix
    /// ([`History::against`]). At half size, a last pass upsamples what it accumulated.
    pub(crate) fn plan(
        graph: &mut Graph,
        depths: [ImageId; 2],
        view: [u32; 2],
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
        let image = graph.scalar("ambient occlusion", view);
        let half = size != view;
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
                let pass = if half { pass } else { pass.overwrite(image) };
                pass.overwrite(accumulated)
            } else {
                pass.write_buffer(terms[(i + 1) % 2])
            };
            pass.add()
        });
        let upsample = half.then(|| {
            let pass = graph.compute(label!("ambient occlusion upsample"));
            let pass = if depths[0] == depths[1] {
                pass.sample(depths[0])
            } else {
                pass.sample(depths[0]).sample(depths[1])
            };
            pass.sample(accumulated).overwrite(image).add()
        });
        Plan {
            size,
            view,
            depths,
            levels,
            terms,
            image,
            accumulated,
            history,
            prefilter,
            occlusion,
            denoises,
            upsample,
        }
    }

    /// Records `plan`'s passes, the frame's next, as `seen` says the frame draws the view.
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
            view: plan.view,
            depths: [0; 2],
            target: 0,
            radius: seen.settings.radius,
            power: FINAL_VALUE_POWER * seen.settings.strength,
            noise: seen.noise,
            blur: BLUR,
            starts: levels(plan.size).0,
            step: plan.step(),
            occluded: 0,
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
                    target: match plan.upsample {
                        Some(_) => 0,
                        None => pass.storage(plan.image, 0),
                    },
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
        if let Some(upsample) = plan.upsample {
            let mut pass = frame.compute(upsample);
            let push = Push {
                depths: plan.depths.map(|depth| pass.sampled(depth)),
                target: pass.storage(plan.image, 0),
                occluded: pass.sampled(plan.accumulated),
                ..base
            };
            pass.set_pipeline(&self.upsample);
            pass.push_constants(bytemuck::bytes_of(&push));
            let [across, down] = plan.view.map(|side| side.div_ceil(GROUP));
            pass.dispatch([across, down, 1]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{DVec3, Quat, Vec3};

    use super::*;
    use crate::taa::jitter;
    use crate::{Projection, Viewpoint};

    #[test]
    fn the_history_moves_with_the_origin() {
        let viewpoint = Viewpoint {
            position: DVec3::new(2.0, -1.0, 5.0),
            rotation: Quat::from_rotation_y(0.3),
            projection: Projection::Perspective {
                fov: 0.8,
                near: 0.1,
            },
        };
        let unjittered = viewpoint.view_projection(1.5);
        let jittered = jitter(Vec2::new(0.001, -0.002)) * unjittered;
        let mut history = History::default();
        history.accumulated(7, [64, 32], jittered, unjittered);
        let by = Vec3::new(1024.0, 0.0, -2048.0);
        history.moved(by);
        let moved = history.against(8, [64, 32], false).expect("accumulated");
        let p = Vec3::new(0.3, 1.2, -3.0);
        let (was, is) = (jittered.project_point3(p), moved.project_point3(p - by));
        assert!(is.abs_diff_eq(was, 1e-4), "{is}, not {was}");
        let moved = history.last.expect("accumulated").unjittered;
        let (was, is) = (unjittered.project_point3(p), moved.project_point3(p - by));
        assert!(is.abs_diff_eq(was, 1e-4), "{is}, not {was}");
    }

    /// View-space X is right, Y up, and depth the distance ahead (-Z), through a perspective
    /// antialiasing moves part of a pixel.
    #[test]
    fn pixels_unproject_to_where_their_points_are() {
        let viewpoint = Viewpoint {
            position: DVec3::new(3.0, 1.0, -2.0),
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
        // The view's top row (V of 0) is up its space; a depth of 0 is infinitely far.
        assert!(unprojected.view_add.y > 0.0, "{}", unprojected.view_add.y);
        assert_eq!(
            unprojected.depth_mul / (unprojected.depth_add + 0.0),
            f32::INFINITY
        );
    }

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

    #[test]
    fn views_past_1080p_occlude_at_half_size_rounded_up() {
        assert_eq!(occlusion_size([1920, 1080], None), [1920, 1080]);
        assert_eq!(occlusion_size([1920, 1081], None), [960, 541]);
        assert_eq!(occlusion_size([3024, 1890], None), [1512, 945]);
        assert_eq!(occlusion_size([3024, 1890], Some(false)), [3024, 1890]);
        assert_eq!(occlusion_size([193, 1], Some(true)), [97, 1]);
    }

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

    /// From a jittered projection's pixels, offset as it was, to a view that moved and turned
    /// since: clip W is the distance ahead of that view.
    #[test]
    fn points_reproject_to_where_the_frame_before_saw_them() {
        let view_from_world = |viewpoint: &Viewpoint| {
            Mat4::from_rotation_translation(viewpoint.rotation, viewpoint.position.as_vec3())
                .inverse()
        };
        let now = Viewpoint {
            position: DVec3::new(3.0, 1.0, -2.0),
            rotation: Quat::from_rotation_y(0.4),
            projection: Projection::Perspective {
                fov: 1.1,
                near: 0.05,
            },
        };
        let before = Viewpoint {
            position: DVec3::new(2.6, 1.2, -1.5),
            rotation: Quat::from_rotation_y(0.32) * Quat::from_rotation_x(-0.05),
            ..now
        };
        let world_from_view = Mat4::from_rotation_translation(now.rotation, now.position.as_vec3());
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

    /// One that stops again cycles through the 8 it stopped among.
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

    /// Not after a skipped frame, a resize, or a cut.
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
