//! Screen-space reflections: what the view shows, seen in the mirror of each opaque point.
//! `shaders/reflections.slang` traces them through the view's depth in compute passes, after the
//! opaque depth and before shading. Shading takes a reflection in place of the sky's specular light
//! where a ray found something, blurred by roughness. Rays trace against the view's colors as the
//! frame before showed them, which a pass of their own keeps each frame, once the view is drawn and
//! resolved.
//!
//! Each ray mirrors its point's shading normal, where an earlier pass found it: from the visibility
//! buffer ([`MeshPass::find_normals`]), or drawn by the model shaders beside their depth
//! ([`Way::Normals`]). Elsewhere it mirrors the depth's normal.
//!
//! [`MeshPass::find_normals`]: crate::mesh::MeshPass::find_normals
//! [`Way::Normals`]: crate::mesh::Way::Normals

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec3};
use gg_core::{Result, label};
use gg_gpu::{
    BufferId, ComputePipeline, Device, Frame, Graph, ImageId, PassId, StorageImage, Target,
    mip_sizes,
};

use crate::ao::Unprojection;

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/reflections.spv"));

/// Pixels across and down per workgroup: `GROUP` in `shaders/reflections.slang`.
const GROUP: u32 = 8;

/// The depth pyramid's most levels: `MOST_LEVELS` in `shaders/reflections.slang`, enough to halve a
/// view 2,048 pixels on its longer side to a texel.
const MOST_LEVELS: usize = 12;

/// The reflections' most levels, each half the last: the last's texels are 64 pixels a side, as
/// wide as shading blurs a reflection. A wider lobe takes the sky's light, by
/// [`Reflections::roughness`].
const LEVELS: u32 = 7;

/// How the view reflects what it shows, as
/// [`RenderData::reflections`](crate::RenderData::reflections) has it. Each opaque point's ray,
/// traced through the view's depth, replaces the sky's specular light where it finds something,
/// blurred by the point's roughness, and leaves the sky's where it finds nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reflections {
    /// How far a ray reaches, in world units, fading over its last quarter. 100 by default. Not
    /// positive and finite: nothing reflects, and none of it runs.
    pub distance: f32,
    /// How far behind the depth a ray may pass and still hit, in world units: how thick what the
    /// depth shows is taken to be, fading with distance. 0.25 by default. Not positive and finite:
    /// nothing reflects, and none of it runs.
    pub thickness: f32,
    /// The roughness past which a surface takes the sky's specular light alone, fading over the
    /// last quarter. 0.6 by default, past which a reflection would blur wider than shading blurs
    /// it. Not positive and finite: nothing reflects, and none of it runs.
    pub roughness: f32,
}

impl Default for Reflections {
    fn default() -> Self {
        Self {
            distance: 100.0,
            thickness: 0.25,
            roughness: 0.6,
        }
    }
}

impl Reflections {
    /// Whether distance, thickness and roughness are all positive and finite.
    pub(crate) fn reflects(&self) -> bool {
        let positive = |value: f32| value.is_finite() && value > 0.0;
        positive(self.distance) && positive(self.thickness) && positive(self.roughness)
    }
}

/// The reflections' part of the frame's data: `Reflected` in `shaders/mesh.slang`, field for field.
/// All 0 for none.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, NoUninit)]
pub(crate) struct ReflectionData {
    colors: u32,
    spread: u32,
    roughness: f32,
    unused: u32,
}

/// `Trace` in `shaders/reflections.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Trace {
    previous_clip_from_view: Mat4,
    view_from_world: Mat4,
    view_mul: [f32; 2],
    view_add: [f32; 2],
    depth_mul: f32,
    depth_add: f32,
    size: [u32; 2],
    colors: u32,
    target: u32,
    spread: u32,
    levels: u32,
    distance: f32,
    thickness: f32,
    starts: [u32; MOST_LEVELS],
    scale: f32,
    normals: [u32; 2],
    unused: [u32; 3],
}

/// `Push` in `shaders/reflections.slang`, field for field, for every pass.
#[repr(C)]
#[derive(Clone, Copy, Default, NoUninit)]
struct Push {
    pyramid: u64,
    trace: u64,
    depths: [u32; 2],
    size: [u32; 2],
    source_size: [u32; 2],
    source: u32,
    target: u32,
}

/// The size of depth pyramid level `level`: the view's halved that many times, rounded up.
fn level_size(size: [u32; 2], level: usize) -> [u32; 2] {
    size.map(|side| side.div_ceil(1 << level))
}

/// The depth pyramid at `size`: levels halving the view, rounded up, until one is a texel or there
/// are [`MOST_LEVELS`]. Returns their count, where each starts in texels, and their total bytes.
fn pyramid(size: [u32; 2]) -> (usize, [u32; MOST_LEVELS], u64) {
    let mut starts = [0; MOST_LEVELS];
    let mut start = 0;
    let mut count = 0;
    for (level, at) in starts.iter_mut().enumerate() {
        let [width, height] = level_size(size, level);
        // Within a texture's largest size squared, plus a third, which a u32 holds.
        *at = start as u32;
        start += u64::from(width) * u64::from(height);
        count = level + 1;
        if width == 1 && height == 1 {
            break;
        }
    }
    (count, starts, start * 4)
}

/// The reflections' levels at `size`: [`LEVELS`], or the mip chain's, if fewer.
fn reflected_levels(size: [u32; 2]) -> u32 {
    (mip_sizes(size[0], size[1]).count() as u32).min(LEVELS)
}

/// The reflections' images, kept from frame to frame: the view's colors as the last reflecting
/// frame kept them, and the view's reflections, in levels. Remade when the view's size changes,
/// dropped by a frame without reflections; either forgets how the last frame drew the kept colors.
/// Apart from [`ReflectionPass`] because a frame takes them as targets while recording.
#[derive(Default)]
pub(crate) struct Images {
    images: Option<[StorageImage; 2]>,
    last: Option<Kept>,
}

/// The last frame that kept the view's colors: its number, the view's size, its world-to-clip
/// matrix without antialiasing's offset (as resolved), and its exposure scale.
#[derive(Clone, Copy)]
struct Kept {
    frame: u64,
    size: [u32; 2],
    clip_from_world: Mat4,
    exposure: f32,
}

impl Images {
    /// Moves what it keeps into the space of a frame whose origin is `by` further along: what was
    /// at `p` is at `p - by`, as [`RenderData::origin`](crate::RenderData::origin) says.
    pub(crate) fn moved(&mut self, by: Vec3) {
        if let Some(last) = &mut self.last {
            let along = Mat4::from_translation(by);
            last.clip_from_world *= along;
        }
    }

    /// Makes its images at `size` if there are none or they're another size, forgetting the kept
    /// colors.
    pub(crate) fn prepare(&mut self, device: &Arc<Device>, size: [u32; 2]) -> Result<()> {
        if (self.images.as_ref()).is_some_and(|images| images[0].size() == size) {
            return Ok(());
        }
        self.forget();
        let kept = StorageImage::new(device, size, 1, false, "reflections kept")?;
        let reflected =
            StorageImage::new(device, size, reflected_levels(size), false, "reflections")?;
        self.images = Some([kept, reflected]);
        Ok(())
    }

    /// Drops its images and the kept colors, as a frame without reflections does. Images made again
    /// hold none until a frame keeps some.
    pub(crate) fn forget(&mut self) {
        // They retire once the GPU is done with them.
        self.images = None;
        self.last = None;
    }

    /// What frame `frame` traces against: the world-to-clip matrix (without antialiasing's offset)
    /// and exposure of the colors the frame before kept, if it did, at `size`.
    pub(crate) fn traced_against(&self, frame: u64, size: [u32; 2]) -> Option<(Mat4, f32)> {
        (self.last)
            .filter(|last| last.frame + 1 == frame && last.size == size)
            .map(|last| (last.clip_from_world, last.exposure))
    }

    /// Notes that submitted frame `frame` kept the view's colors at `size`, drawn by
    /// `clip_from_world` (without antialiasing's offset) at `exposure`, for the next frame's
    /// reflections.
    pub(crate) fn kept(
        &mut self,
        frame: u64,
        size: [u32; 2],
        clip_from_world: Mat4,
        exposure: f32,
    ) {
        self.last = Some(Kept {
            frame,
            size,
            clip_from_world,
            exposure,
        });
    }

    /// Notes that submitted frame `frame` reflected the kept colors but kept none, as a frame that
    /// shows something in place of the view's light doesn't. The next frame then takes the same
    /// colors, which nothing has overwritten.
    pub(crate) fn carry(&mut self, frame: u64) {
        if let Some(last) = &mut self.last {
            last.frame = frame;
        }
    }

    /// Its images as targets, at the indices [`ReflectionPass::plan`] and
    /// [`ReflectionPass::plan_keep`] give them. None before [`prepare`](Self::prepare).
    pub(crate) fn targets(&mut self) -> impl Iterator<Item = Target<'_>> {
        (self.images.iter_mut()).flat_map(|images| images.iter_mut().map(Target::Storage))
    }
}

/// Traces the view's reflections and keeps its colors: depth pyramid (first level and the rest),
/// trace, blur, and keep.
pub(crate) struct ReflectionPass {
    depth: ComputePipeline,
    reduce: ComputePipeline,
    trace: ComputePipeline,
    blur: ComputePipeline,
    keep: ComputePipeline,
}

/// A frame's reflections in its graph.
pub(crate) struct Plan {
    size: [u32; 2],
    depths: [ImageId; 2],
    normals: [Option<ImageId>; 2],
    pyramid: BufferId,
    levels: [Option<PassId>; MOST_LEVELS],
    kept: ImageId,
    reflected: ImageId,
    spread: ImageId,
    trace: PassId,
    blurs: [Option<PassId>; LEVELS as usize - 1],
}

impl Plan {
    /// What shading samples: the reflections and how far each spreads.
    pub(crate) fn sampled(&self) -> [ImageId; 2] {
        [self.reflected, self.spread]
    }

    /// The reflections' part of the frame's data, for `settings`, through the indices `sampled`
    /// gives (a sampling pass's).
    pub(crate) fn data(
        &self,
        sampled: impl Fn(ImageId) -> u32,
        settings: &Reflections,
    ) -> ReflectionData {
        ReflectionData {
            colors: sampled(self.reflected),
            spread: sampled(self.spread),
            roughness: settings.roughness,
            unused: 0,
        }
    }
}

/// A frame's keeping of the view's colors in its graph.
pub(crate) struct Keep {
    pass: PassId,
    shown: ImageId,
    kept: ImageId,
    size: [u32; 2],
}

/// How a frame draws the view, for its reflections: `clip_from_view` is a perspective, with
/// antialiasing's offset. Also the kept colors' clip space, from [`Images::traced_against`], and
/// their rescale to this frame's exposure (its exposure over theirs).
pub(crate) struct Seen {
    pub(crate) clip_from_view: Mat4,
    pub(crate) world_from_view: Mat4,
    pub(crate) kept_clip_from_world: Mat4,
    pub(crate) kept_scale: f32,
    pub(crate) settings: Reflections,
}

impl ReflectionPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        Ok(Self {
            depth: pipeline("reflections depth", c"depth_main")?,
            reduce: pipeline("reflections depth levels", c"reduce_main")?,
            trace: pipeline("reflections", c"trace_main")?,
            blur: pipeline("reflections blur", c"blur_main")?,
            keep: pipeline("reflections keep", c"keep_main")?,
        })
    }

    /// Declares in `graph` the reflections at `size`, from `depths` (the nearest is what's opaque,
    /// the first the visibility buffer's where there is one) and the shading `normals` an earlier
    /// pass found. They trace against the colors the frame before kept in the [`Images`]' first, at
    /// target `first`, into its second, at `first + 1`. The passes:
    /// - the depth pyramid's levels, in a buffer of the graph's;
    /// - the trace, into the reflections' first level, and how far each spreads, into an image of
    ///   the graph's;
    /// - the blurs of the levels after.
    pub(crate) fn plan(
        graph: &mut Graph,
        first: usize,
        depths: [ImageId; 2],
        normals: [Option<ImageId>; 2],
        size: [u32; 2],
    ) -> Plan {
        // Each depth once; the two may be one image.
        let distinct = &depths[..if depths[0] == depths[1] { 1 } else { 2 }];
        let (count, _, bytes) = pyramid(size);
        let pyramid = graph.buffer("reflections depth pyramid", bytes);
        let mut levels = [None; MOST_LEVELS];
        for (i, level) in levels.iter_mut().take(count).enumerate() {
            let pass = graph.compute(label!("reflections depth"));
            let pass = match i {
                0 => distinct
                    .iter()
                    .fold(pass, |pass, &depth| pass.sample(depth)),
                _ => pass,
            };
            *level = Some(pass.write_buffer(pyramid).add());
        }
        let kept = graph.target(first);
        let reflected = graph.target(first + 1);
        let spread = graph.scalar("reflections spread", size);
        let trace = (graph.compute(label!("reflections")))
            .read_buffer(pyramid)
            .sample(kept);
        let sampled = distinct.iter().chain(normals.iter().flatten());
        let trace = sampled.fold(trace, |trace, &image| trace.sample(image));
        let trace = trace.overwrite(reflected).overwrite(spread).add();
        let mut blurs = [None; LEVELS as usize - 1];
        for blur in blurs.iter_mut().take(reflected_levels(size) as usize - 1) {
            *blur = Some(
                graph
                    .compute(label!("reflections blur"))
                    .write(reflected)
                    .add(),
            );
        }
        Plan {
            size,
            depths,
            normals,
            pyramid,
            levels,
            kept,
            reflected,
            spread,
            trace,
            blurs,
        }
    }

    /// Declares in `graph` the keeping of `shown`, the view's drawn and resolved colors at `size`,
    /// into the [`Images`]' first, at target `first`, for the next frame's reflections.
    pub(crate) fn plan_keep(
        graph: &mut Graph,
        first: usize,
        shown: ImageId,
        size: [u32; 2],
    ) -> Keep {
        let kept = graph.target(first);
        let pass = (graph.compute(label!("reflections keep")))
            .sample(shown)
            .overwrite(kept)
            .add();
        Keep {
            pass,
            shown,
            kept,
            size,
        }
    }

    /// Records `plan`'s passes, the frame's next, as `seen` says the frame draws the view.
    pub(crate) fn record(&self, frame: &mut Frame<'_>, plan: &Plan, seen: &Seen) -> Result<()> {
        let (count, starts, _) = pyramid(plan.size);
        let dispatch = |size: [u32; 2]| [size[0].div_ceil(GROUP), size[1].div_ceil(GROUP), 1];
        for (level, &pass) in plan.levels.iter().flatten().enumerate() {
            let mut pass = frame.compute(pass);
            let size = level_size(plan.size, level);
            let push = match level {
                0 => Push {
                    pyramid: pass.address(plan.pyramid),
                    depths: plan.depths.map(|depth| pass.sampled(depth)),
                    size,
                    ..Push::default()
                },
                _ => Push {
                    pyramid: pass.address(plan.pyramid),
                    size,
                    source_size: level_size(plan.size, level - 1),
                    source: starts[level - 1],
                    target: starts[level],
                    ..Push::default()
                },
            };
            pass.set_pipeline(if level == 0 {
                &self.depth
            } else {
                &self.reduce
            });
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch(dispatch(size));
        }
        let Unprojection {
            view_mul,
            view_add,
            depth_mul,
            depth_add,
        } = Unprojection::new(seen.clip_from_view);
        // The shader's view space has Z ahead; the world's views look down -Z.
        let ahead = Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0));
        let mut pass = frame.compute(plan.trace);
        let normals =
            (plan.normals).map(|normals| normals.map_or(0, |normals| pass.sampled(normals)));
        let trace = Trace {
            previous_clip_from_view: seen.kept_clip_from_world * seen.world_from_view * ahead,
            view_from_world: ahead * seen.world_from_view.inverse(),
            view_mul: view_mul.to_array(),
            view_add: view_add.to_array(),
            depth_mul,
            depth_add,
            size: plan.size,
            colors: pass.sampled(plan.kept),
            target: pass.storage(plan.reflected, 0),
            spread: pass.storage(plan.spread, 0),
            levels: count as u32,
            distance: seen.settings.distance,
            thickness: seen.settings.thickness,
            starts,
            scale: seen.kept_scale,
            normals,
            unused: [0; 3],
        };
        let push = Push {
            pyramid: pass.address(plan.pyramid),
            trace: pass.upload(bytemuck::bytes_of(&trace))?.address(),
            depths: plan.depths.map(|depth| pass.sampled(depth)),
            size: plan.size,
            ..Push::default()
        };
        pass.set_pipeline(&self.trace);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(dispatch(plan.size));
        drop(pass);
        let mut finer = plan.size;
        for (level, (&blur, size)) in (1..).zip(
            plan.blurs
                .iter()
                .flatten()
                .zip(mip_sizes(plan.size[0], plan.size[1]).skip(1)),
        ) {
            let mut pass = frame.compute(blur);
            let push = Push {
                size,
                source_size: finer,
                source: pass.storage(plan.reflected, level - 1),
                target: pass.storage(plan.reflected, level),
                ..Push::default()
            };
            pass.set_pipeline(&self.blur);
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch(dispatch(size));
            finer = size;
        }
        Ok(())
    }

    /// Records `keep`'s pass, the frame's next. Once submitted, the frame notes it with
    /// [`Images::kept`].
    pub(crate) fn keep(&self, frame: &mut Frame<'_>, keep: &Keep) {
        let mut pass = frame.compute(keep.pass);
        let push = Push {
            size: keep.size,
            source: pass.sampled(keep.shown),
            target: pass.storage(keep.kept, 0),
            ..Push::default()
        };
        pass.set_pipeline(&self.keep);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = keep.size.map(|side| side.div_ceil(GROUP));
        pass.dispatch([across, down, 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kept_view_moves_with_the_origin() {
        let viewpoint = crate::Viewpoint {
            position: gg_core::math::DVec3::new(2.0, -1.0, 5.0),
            rotation: gg_core::math::Quat::from_rotation_y(0.3),
            projection: crate::Projection::Perspective {
                fov: 0.8,
                near: 0.1,
            },
        };
        let clip_from_world = viewpoint.view_projection(1.5);
        let mut images = Images::default();
        images.kept(7, [64, 32], clip_from_world, 0.5);
        let by = Vec3::new(1024.0, 0.0, -2048.0);
        images.moved(by);
        let (moved, exposure) = images.traced_against(8, [64, 32]).expect("kept");
        assert_eq!(exposure, 0.5);
        let p = Vec3::new(0.3, 1.2, -3.0);
        let (was, is) = (
            clip_from_world.project_point3(p),
            moved.project_point3(p - by),
        );
        assert!(is.abs_diff_eq(was, 1e-4), "{is}, not {was}");
    }

    /// Back to back, as the shader reads them, no more than its most.
    #[test]
    fn the_pyramid_halves_the_view_to_a_texel() {
        let (count, starts, bytes) = pyramid([5, 3]);
        // 5 by 3, 3 by 2, 2 by 1, 1 by 1.
        assert_eq!(count, 4);
        assert_eq!(starts[..4], [0, 15, 21, 23]);
        assert_eq!(bytes, 24 * 4);
        assert_eq!(pyramid([1, 1]).0, 1);
        let (count, starts, _) = pyramid([1280, 720]);
        assert_eq!(count, 12);
        assert_eq!(starts[1], 1280 * 720);
        assert_eq!(level_size([1280, 720], 11), [1, 1]);
        assert_eq!(pyramid([4096, 4096]).0, MOST_LEVELS);
        assert_eq!(level_size([1280, 720], 4), [80, 45]);
        assert_eq!(level_size([1281, 721], 4), [81, 46]);
        // As many as blur to 64 pixels, or as the view's chain has.
        assert_eq!(reflected_levels([1280, 720]), LEVELS);
        assert_eq!(reflected_levels([4, 16]), 5);
    }

    /// Only at the same size, with the exposure they were kept at. Not once the images go, as for a
    /// frame without reflections or a resize, even if the next frame number comes: images made
    /// again hold nothing.
    #[test]
    fn colors_are_traced_against_by_the_next_frame_alone_until_their_images_go() {
        let mut images = Images::default();
        let drawn = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(images.traced_against(1, [8, 4]), None);
        images.kept(5, [8, 4], drawn, 0.25);
        assert_eq!(images.traced_against(6, [8, 4]), Some((drawn, 0.25)));
        assert_eq!(images.traced_against(7, [8, 4]), None);
        assert_eq!(images.traced_against(5, [8, 4]), None);
        assert_eq!(images.traced_against(6, [8, 5]), None);
        images.forget();
        assert_eq!(images.traced_against(6, [8, 4]), None);
    }

    #[test]
    fn only_a_positive_finite_distance_thickness_and_roughness_reflect() {
        assert!(Reflections::default().reflects());
        let base = Reflections::default();
        for broken in [
            Reflections {
                distance: 0.0,
                ..base
            },
            Reflections {
                distance: f32::INFINITY,
                ..base
            },
            Reflections {
                thickness: -1.0,
                ..base
            },
            Reflections {
                thickness: f32::NAN,
                ..base
            },
            Reflections {
                roughness: 0.0,
                ..base
            },
        ] {
            assert!(!broken.reflects(), "{broken:?}");
        }
    }
}
