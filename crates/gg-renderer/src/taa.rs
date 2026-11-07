//! Temporal antialiasing: the game's view drawn a fraction of a pixel off each frame, by a cycle of
//! offsets, and resolved, before it's tonemapped, against what the frames before it resolved,
//! taken back to where each pixel's point was by its motion and clipped to its neighborhood, as
//! `shaders/taa.slang` says; into one of two images of the view's size, which the next frame
//! resolves against in turn.

use std::iter;
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2, Vec4};
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, PassId, StorageImage, Target};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/taa.spv"));

/// How many pixels across and down a workgroup takes: `GROUP` in `shaders/taa.slang`.
const GROUP: u32 = 8;

/// How the game's view is antialiased, as
/// [`RenderData::antialiasing`](crate::RenderData::antialiasing) names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Antialiasing {
    /// Temporal antialiasing: the view drawn a fraction of a pixel off each frame, by a cycle of
    /// 8 offsets, and each pixel blended with what the frames before resolved where its point was,
    /// as its motion says, clipped to what's round it this frame, so that its edges average what's
    /// drawn across their pixels; what's drawn on the canvas, after it, isn't. Each frame draws
    /// what's opaque in the view with how far it moved in the world since the frame before: a
    /// model's by its instance's transform the frame before, of the world's opaque meshes, and a
    /// game's shader's as it returns it, and the rest with the camera alone.
    #[default]
    Taa,
    /// None: each pixel shows what's drawn at its center.
    None,
}

impl Antialiasing {
    /// Each way, in the order of their indices.
    pub const ALL: [Self; 2] = [Self::Taa, Self::None];
}

/// How many offsets the view cycles through, of the Halton sequence's first.
const OFFSETS: u64 = 8;

/// The `index`th number of the Halton sequence of base `base`: its digits in that base, mirrored
/// about the point, from 0 to 1.
fn halton(mut index: u64, base: u64) -> f32 {
    let (mut fraction, mut result) = (1.0, 0.0);
    while index > 0 {
        fraction /= base as f32;
        result += fraction * (index % base) as f32;
        index /= base;
    }
    result
}

/// How far the `resolved`th frame to resolve the view draws it off, from 0, in pixels across and
/// down, each from -0.5 to 0.5: the Halton sequence of bases 2 and 3, from its first number, of
/// which the frames cycle through [`OFFSETS`], spread evenly across a pixel.
fn offset(resolved: u64) -> Vec2 {
    let index = resolved % OFFSETS + 1;
    Vec2::new(halton(index, 2), halton(index, 3)) - 0.5
}

/// The matrix that moves what clip space holds `offset` across and down in the view's normalized
/// coordinates, whatever its depth: each point's X and Y by the offset times its W.
pub(crate) fn jitter(offset: Vec2) -> Mat4 {
    Mat4::from_cols(
        Vec4::X,
        Vec4::Y,
        Vec4::Z,
        Vec4::new(offset.x, offset.y, 0.0, 1.0),
    )
}

/// `offset`, in pixels of a view `size` pixels, in the view's normalized coordinates, two units
/// across each way.
pub(crate) fn normalized(offset: Vec2, size: [u32; 2]) -> Vec2 {
    let [width, height] = size.map(|side| side as f32);
    offset * 2.0 / Vec2::new(width, height)
}

/// What the resolve reads: `Resolve` in `shaders/taa.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Resolve {
    world_from_clip: Mat4,
    previous_clip_from_world: Mat4,
    jitter: [f32; 2],
    size: [u32; 2],
    colors: u32,
    depth: u32,
    motion: u32,
    history: u32,
    target: u32,
    resolved: u32,
    has_history: u32,
    writes_resolved: u32,
}

/// What the resolve pushes: `Push` in `shaders/taa.slang`.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct ResolvePush {
    resolve: u64,
}

/// The last frame that resolved the view: its number, the view's size, and the matrix from the
/// world to the view's clip space it drew by, without its offset.
#[derive(Clone, Copy)]
struct Resolved {
    frame: u64,
    size: [u32; 2],
    clip_from_world: Mat4,
}

/// What resolves the game's view, how many frames have, which the next one's offset follows, and
/// how the last that did drew it.
pub(crate) struct TaaPass {
    resolve: ComputePipeline,
    resolved: u64,
    last: Option<Resolved>,
}

/// What the game's view resolves into and against: two images of its size, made again as it
/// changes, each frame resolving into the one the frame before didn't, against the other, which
/// frames that resolve take as their targets; apart from [`TaaPass`], as a frame holds them while
/// it records the resolve.
#[derive(Default)]
pub(crate) struct History {
    images: Option<[StorageImage; 2]>,
}

impl History {
    /// Makes its images again at `size`, if they're another size, or there are none yet; the
    /// frame resolves against neither then, as the last that resolved did so at another size.
    pub(crate) fn prepare(&mut self, device: &Arc<Device>, size: [u32; 2]) -> Result<()> {
        if (self.images.as_ref()).is_some_and(|images| images[0].size() == size) {
            return Ok(());
        }
        // Those there retire until the GPU is done with them.
        self.images = None;
        let image = |name| StorageImage::new(device, size, 1, false, name);
        self.images = Some([image("taa history 0")?, image("taa history 1")?]);
        Ok(())
    }

    /// Its images, as a frame that resolves takes them as targets, at the indices
    /// [`TaaPass::plan`] gives them: none before [`prepare`](Self::prepare) has made them.
    pub(crate) fn targets(&mut self) -> impl Iterator<Item = Target<'_>> {
        (self.images.iter_mut()).flat_map(|images| images.iter_mut().map(Target::Storage))
    }
}

/// A frame's resolve in its graph: its pass; the view's size, and its colors, depth and motion, as
/// its passes drew them; the image it resolves into, and the one it resolves against, if the frame
/// before resolved one it can take; and the view's resolved colors, if it writes them apart, for
/// its canvas to be drawn over.
pub(crate) struct Plan {
    pass: PassId,
    size: [u32; 2],
    drawn: [ImageId; 3],
    target: ImageId,
    history: Option<ImageId>,
    resolved: Option<ImageId>,
}

impl Plan {
    /// What the view shows, resolved: its resolved colors, if they're written apart, and otherwise
    /// what the frame resolved into.
    pub(crate) fn shown(&self) -> ImageId {
        self.resolved.unwrap_or(self.target)
    }
}

/// How a frame drew its view: from the world to its clip space, without its offset and with it,
/// and the offset, in its normalized coordinates.
#[derive(Clone, Copy)]
pub(crate) struct Drawn {
    pub(crate) unjittered: Mat4,
    pub(crate) clip_from_world: Mat4,
    pub(crate) jitter: Vec2,
}

impl TaaPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        Ok(Self {
            resolve: ComputePipeline::new(device, "taa resolve", SPIRV, c"resolve_main")?,
            resolved: 0,
            last: None,
        })
    }

    /// How far the next frame to resolve the view draws it off, in pixels across and down, as
    /// [`offset`] says: by the frames that have resolved it, rather than by the frames' numbers, so
    /// that a run's offsets follow from what it antialiased alone.
    pub(crate) fn offset(&self) -> Vec2 {
        offset(self.resolved)
    }

    /// Declares in `graph` the resolve of frame `frame`'s view, of `size` pixels, from its colors,
    /// depth and motion, `drawn`, into the image of a [`History`]'s two, at targets `first` and 1
    /// past it, that the frame before didn't resolve into, against the other, if that one resolved
    /// the view at its size; and into the view's resolved colors too, made in `graph`, if `apart`.
    pub(crate) fn plan(
        &self,
        graph: &mut Graph,
        first: usize,
        frame: u64,
        size: [u32; 2],
        drawn: [ImageId; 3],
        apart: bool,
    ) -> Plan {
        let images = [graph.target(first), graph.target(first + 1)];
        let target = images[(frame % 2) as usize];
        let history = (self.last)
            .filter(|last| last.frame + 1 == frame && last.size == size)
            .map(|_| images[((frame + 1) % 2) as usize]);
        let resolved = apart.then(|| graph.hdr("view resolved", size));
        let pass = graph.compute(label!("taa resolve"));
        let pass = drawn
            .into_iter()
            .chain(history)
            .fold(pass, |pass, image| pass.sample(image));
        let pass = iter::once(target)
            .chain(resolved)
            .fold(pass, |pass, image| pass.overwrite(image));
        Plan {
            pass: pass.add(),
            size,
            drawn,
            target,
            history,
            resolved,
        }
    }

    /// Records `plan`'s resolve of its view, as `drawn` says the frame drew it, against what the
    /// frame before resolved, as it noted how it drew it, and notes how this one did, for the next.
    pub(crate) fn resolve(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &Plan,
        drawn: Drawn,
    ) -> Result<()> {
        let number = frame.number();
        let mut pass = frame.compute(plan.pass);
        let previous = (self.last.filter(|_| plan.history.is_some()))
            .map_or(drawn.unjittered, |last| last.clip_from_world);
        let [colors, depth, motion] = plan.drawn.map(|image| pass.sampled(image));
        let resolve = Resolve {
            world_from_clip: drawn.clip_from_world.inverse(),
            previous_clip_from_world: previous,
            jitter: drawn.jitter.to_array(),
            size: plan.size,
            colors,
            depth,
            motion,
            history: plan.history.map_or(0, |history| pass.sampled(history)),
            target: pass.storage(plan.target, 0),
            resolved: plan
                .resolved
                .map_or(0, |resolved| pass.storage(resolved, 0)),
            has_history: u32::from(plan.history.is_some()),
            writes_resolved: u32::from(plan.resolved.is_some()),
        };
        let push = ResolvePush {
            resolve: pass.upload(bytemuck::bytes_of(&resolve))?.address(),
        };
        pass.set_pipeline(&self.resolve);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = plan.size.map(|side| side.div_ceil(GROUP));
        pass.dispatch([across, down, 1]);
        self.resolved += 1;
        self.last = Some(Resolved {
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

    /// The offsets cycle through the Halton sequence's first eight of bases 2 and 3, within half a
    /// pixel each way, their mean the pixel's center.
    #[test]
    fn offsets_cycle_through_eight_across_a_pixel() {
        let offsets: Vec<Vec2> = (0..8).map(offset).collect();
        assert_eq!(offsets[0], Vec2::new(0.0, 1.0 / 3.0 - 0.5));
        assert_eq!(offsets[1], Vec2::new(-0.25, 2.0 / 3.0 - 0.5));
        assert_eq!(offset(8), offsets[0]);
        let mean = offsets.iter().sum::<Vec2>() / 8.0;
        assert!(mean.abs().max_element() < 0.07, "{mean}");
        for offset in &offsets {
            assert!(offset.abs().max_element() <= 0.5, "{offset}");
        }
        let distinct = |i: usize| offsets[..i].iter().all(|before| *before != offsets[i]);
        assert!((0..8).all(distinct));
    }

    /// A point moves across and down by the offset, in normalized coordinates, whatever its
    /// depth, under perspective and without it.
    #[test]
    fn jitter_moves_every_depth_alike() {
        let offset = normalized(Vec2::new(0.25, -0.5), [400, 200]);
        assert_eq!(offset, Vec2::new(0.00125, -0.005));
        let moved = jitter(offset);
        for clip in [
            Vec4::new(0.3, -0.2, 0.5, 1.0),
            Vec4::new(-4.0, 2.0, 0.1, 8.0),
        ] {
            let at = moved * clip;
            let ndc = |v: Vec4| Vec2::new(v.x, v.y) / v.w;
            assert!((ndc(at) - ndc(clip) - offset).abs().max_element() < 1.0e-6);
            assert_eq!((at.z, at.w), (clip.z, clip.w));
        }
    }
}
