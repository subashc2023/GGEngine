//! Screen-space reflections: what the game's view shows of the world, seen in the mirror of each
//! point of what's opaque in it, as `shaders/reflections.slang` traces it through the view's depth,
//! in compute passes once what's opaque has drawn its depth and before it's shaded, which takes the
//! reflection in place of the sky's specular light where a ray found something, blurred as its
//! roughness spreads it; from the view's colors as the frame before showed them, which a pass of its
//! own keeps each frame, once the view is drawn and resolved, in an image of the view's size.

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

/// How many pixels across and down a workgroup takes: `GROUP` in `shaders/reflections.slang`.
const GROUP: u32 = 8;

/// The most levels the depth pyramid has: `MOST_LEVELS` in `shaders/reflections.slang`, as many as
/// halve a view 2,048 pixels on its longer side to a texel.
const MOST_LEVELS: usize = 12;

/// The most levels the reflections have, each half the one before: their last's texels 64 pixels a
/// side, as wide as shading blurs a reflection. Past that, a lobe wide enough to blur more takes the
/// sky's light, by [`Reflections::roughness`].
const LEVELS: u32 = 7;

/// How the game's view reflects what it shows, as
/// [`RenderData::reflections`](crate::RenderData::reflections) has it: each point of what's opaque
/// seen in its mirror, by a ray traced through the view's depth, which takes the place of the sky's
/// specular light where it found something, blurred as the point's roughness spreads it, and leaves
/// the sky's where it found nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reflections {
    /// How far a ray reaches, in the world's units, fading over its last quarter: 100 by default. A
    /// distance that isn't positive and finite reflects nothing, and none of it runs.
    pub distance: f32,
    /// How far behind what the view's depth shows a ray may pass and still find it, in the world's
    /// units, as thick as what the depth shows is taken to be, fading as it's farther: 0.25 by
    /// default. A thickness that isn't positive and finite reflects nothing, and none of it runs.
    pub thickness: f32,
    /// The roughness past which a surface takes the sky's specular light alone, fading over its
    /// last quarter: 0.6 by default, past which a reflection blurs wider than shading blurs it. A
    /// roughness that isn't positive and finite reflects nothing, and none of it runs.
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
    /// Whether it reflects anything: its distance, thickness and roughness are positive and finite.
    pub(crate) fn reflects(&self) -> bool {
        let positive = |value: f32| value.is_finite() && value > 0.0;
        positive(self.distance) && positive(self.thickness) && positive(self.roughness)
    }
}

/// The reflections' part of the frame's data, `Reflected` in `shaders/mesh.slang`, field for field:
/// the slots in the bindless set of the view's reflections, in levels, and of how far each spreads,
/// and the roughness past which a surface takes none; all 0 for none.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, NoUninit)]
pub(crate) struct ReflectionData {
    colors: u32,
    spread: u32,
    roughness: f32,
    unused: u32,
}

/// What the trace reads: `Trace` in `shaders/reflections.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Trace {
    previous_clip_from_view: Mat4,
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
    unused: u32,
}

/// What every pass pushes: `Push` in `shaders/reflections.slang`, field for field.
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

/// The size of the depth pyramid's level `level` of a view of `size` pixels: the view's halved that
/// many times, rounded up.
fn level_size(size: [u32; 2], level: usize) -> [u32; 2] {
    size.map(|side| side.div_ceil(1 << level))
}

/// The depth pyramid of a view of `size` pixels, its first level the view's size, each after half
/// the one before, rounded up, until a level is a texel or [`MOST_LEVELS`]: how many levels it has,
/// where each starts among them all, in texels, and their bytes together.
fn pyramid(size: [u32; 2]) -> (usize, [u32; MOST_LEVELS], u64) {
    let mut starts = [0; MOST_LEVELS];
    let mut start = 0;
    let mut count = 0;
    for (level, at) in starts.iter_mut().enumerate() {
        let [width, height] = level_size(size, level);
        // Within a texture's largest size squared, and a third of it again, which a u32 holds.
        *at = start as u32;
        start += u64::from(width) * u64::from(height);
        count = level + 1;
        if width == 1 && height == 1 {
            break;
        }
    }
    (count, starts, start * 4)
}

/// How many levels the reflections of a view of `size` pixels have: [`LEVELS`], or as many as its
/// mip chain has, if fewer.
fn reflected_levels(size: [u32; 2]) -> u32 {
    (mip_sizes(size[0], size[1]).count() as u32).min(LEVELS)
}

/// The reflections' images, which last from frame to frame: the view's colors as the last frame
/// that reflected kept them, and the view's reflections, in levels, made again as the view's size
/// changes, and dropped by a frame that renders the view without reflections; and how the last
/// frame that kept colors in them drew them, which their remaking or dropping forgets. Apart from
/// [`ReflectionPass`], as a frame takes them as targets while it records the passes.
#[derive(Default)]
pub(crate) struct Images {
    images: Option<[StorageImage; 2]>,
    last: Option<Kept>,
}

/// The last frame that kept the view's colors: its number, the view's size, the matrix from the
/// world to the view's clip space it drew by, without its offset, as they were resolved, and what
/// lit shading multiplied light by in it.
#[derive(Clone, Copy)]
struct Kept {
    frame: u64,
    size: [u32; 2],
    clip_from_world: Mat4,
    exposure: f32,
}

impl Images {
    /// Makes its images at `size`, if they're another size, or there are none, forgetting the
    /// colors kept in those there.
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

    /// Lets its images go, as a frame that renders the view without reflections does, and the
    /// colors kept in them: those made again hold none until a frame keeps some.
    pub(crate) fn forget(&mut self) {
        // They retire until the GPU is done with them.
        self.images = None;
        self.last = None;
    }

    /// The matrix from the world to the view's clip space, without antialiasing's offset, that the
    /// colors kept for frame `frame` were drawn by, if the frame before it kept them, of a view of
    /// `size` pixels, in the images there are, and the exposure they were drawn at: what its
    /// reflections are traced against.
    pub(crate) fn traced_against(&self, frame: u64, size: [u32; 2]) -> Option<(Mat4, f32)> {
        (self.last)
            .filter(|last| last.frame + 1 == frame && last.size == size)
            .map(|last| (last.clip_from_world, last.exposure))
    }

    /// Notes that frame `frame`, submitted, kept the view's colors, of `size` pixels, drawn
    /// without antialiasing's offset by `clip_from_world`, at `exposure`, for the next frame's
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

    /// Its images, as a frame that reflects takes them as targets, at the indices
    /// [`ReflectionPass::plan`] and [`ReflectionPass::plan_keep`] give them: none before
    /// [`prepare`](Self::prepare) has made them.
    pub(crate) fn targets(&mut self) -> impl Iterator<Item = Target<'_>> {
        (self.images.iter_mut()).flat_map(|images| images.iter_mut().map(Target::Storage))
    }
}

/// What traces the view's reflections and keeps its colors for them: the depth pyramid's first
/// level and those after, the trace, the blur, and the keeping.
pub(crate) struct ReflectionPass {
    depth: ComputePipeline,
    reduce: ComputePipeline,
    trace: ComputePipeline,
    blur: ComputePipeline,
    keep: ComputePipeline,
}

/// A frame's reflections in its graph: the view's size, the depths whose nearer is what's opaque,
/// the depth pyramid and its passes, the colors the frame before kept, the view's reflections, in
/// levels, and how far each spreads, which the passes that shade the view sample, and the trace
/// and the blurs.
pub(crate) struct Plan {
    size: [u32; 2],
    depths: [ImageId; 2],
    pyramid: BufferId,
    levels: [Option<PassId>; MOST_LEVELS],
    kept: ImageId,
    reflected: ImageId,
    spread: ImageId,
    trace: PassId,
    blurs: [Option<PassId>; LEVELS as usize - 1],
}

impl Plan {
    /// What the passes that shade the view sample of it: its reflections and how far each spreads.
    pub(crate) fn sampled(&self) -> [ImageId; 2] {
        [self.reflected, self.spread]
    }

    /// The reflections' part of the frame's data, sampled through the indices `sampled` gives, a
    /// pass's that samples them, as `settings` says.
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

/// A frame's keeping of the view's colors in its graph: its pass, the colors it keeps, and where,
/// and the view's size.
pub(crate) struct Keep {
    pass: PassId,
    shown: ImageId,
    kept: ImageId,
    size: [u32; 2],
}

/// What a frame's reflections take of how it draws the view: from the view's space to its clip space,
/// with antialiasing's offset, under perspective; from the view's space to the world; from the world
/// to the clip space the colors it traces against were drawn in, as
/// [`Images::traced_against`] gives it; what those colors are multiplied by to be of this frame's
/// light, its exposure over theirs, as an adapting exposure changes; and the settings.
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

    /// Declares in `graph` the reflections of a view of `size` pixels, from `depths`, as the passes
    /// before drew them, whose nearer at each pixel is what's opaque there, against the colors the
    /// frame before kept in the [`Images`]' first, at target `first`, into its second, at 1 past it:
    /// the depth pyramid's levels, in a buffer of the graph's own; the trace, into the reflections'
    /// first level, and how far each spreads, into an image of the graph's own; and the blurs of the
    /// levels after.
    pub(crate) fn plan(
        graph: &mut Graph,
        first: usize,
        depths: [ImageId; 2],
        size: [u32; 2],
    ) -> Plan {
        let (count, _, bytes) = pyramid(size);
        let pyramid = graph.buffer("reflections depth pyramid", bytes);
        let mut levels = [None; MOST_LEVELS];
        for (i, level) in levels.iter_mut().take(count).enumerate() {
            let pass = graph.compute(label!("reflections depth"));
            let pass = match i {
                0 if depths[0] == depths[1] => pass.sample(depths[0]),
                0 => pass.sample(depths[0]).sample(depths[1]),
                _ => pass,
            };
            *level = Some(pass.write_buffer(pyramid).add());
        }
        let kept = graph.target(first);
        let reflected = graph.target(first + 1);
        let spread = graph.scalar("reflections spread", size);
        let trace = (graph.compute(label!("reflections")))
            .read_buffer(pyramid)
            .sample(kept)
            .overwrite(reflected)
            .overwrite(spread)
            .add();
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
            pyramid,
            levels,
            kept,
            reflected,
            spread,
            trace,
            blurs,
        }
    }

    /// Declares in `graph` the keeping of `shown`, the view's colors, of `size` pixels, as drawn and
    /// resolved, into the [`Images`]' first, at target `first`, for the next frame's reflections.
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

    /// Records `plan`'s passes, the next of the frame's, as `seen` says the frame draws the view,
    /// against the colors the frame before kept.
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
        // The view's space has Z ahead, as the shader's does, where the world's views look down -Z.
        let ahead = Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0));
        let mut pass = frame.compute(plan.trace);
        let trace = Trace {
            previous_clip_from_view: seen.kept_clip_from_world * seen.world_from_view * ahead,
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
            unused: 0,
        };
        let push = Push {
            pyramid: pass.address(plan.pyramid),
            trace: pass.upload(bytemuck::bytes_of(&trace))?.address(),
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

    /// Records `keep`'s pass, the next of the frame's, which the frame, once submitted, notes in
    /// the [`Images`] it keeps the colors in, as [`Images::kept`] says.
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

    /// The pyramid's levels halve the view, rounded up, to a texel, one after another, as the shader
    /// reads them, as many as that takes, but no more than its most.
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
        // The reflections' levels: as many as blur to 64 pixels, or as the view's chain has.
        assert_eq!(reflected_levels([1280, 720]), LEVELS);
        assert_eq!(reflected_levels([4, 16]), 5);
    }

    /// A frame traces against the colors the frame just before it kept, at its size alone, knowing
    /// the exposure they were kept at; and not once the images are let go of, as a frame that
    /// renders the view without reflections does, and as making them again at another size does
    /// first, though a frame of the next number comes, as after a frame that had nothing to render
    /// into: the images made again hold nothing.
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

    /// Settings that would reflect nothing, or reflect as no number, run none of it.
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
