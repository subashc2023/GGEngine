//! Temporal antialiasing (`shaders/taa.slang`): the view drawn off by a cycle of subpixel offsets,
//! then, before tonemapping, resolved against the frames before, reprojected by motion and clipped
//! to each pixel's neighborhood. A [`History`] holds three pairs of images, each frame writing one
//! of a pair and reading the other:
//!
//! - the resolved colors;
//! - what each pixel's offsets drew, keeping a still pixel's history unclipped where only the
//!   offsets change it;
//! - the flicker test's, keeping the luminance of a pixel that flickers as the view moves.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, PassId, StorageImage, Target};

use crate::exposure::rescale;

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/taa.spv"));

/// Pixels across and down per workgroup: `GROUP` in `shaders/taa.slang`.
const GROUP: u32 = 8;

/// How the view is antialiased, as [`RenderData::antialiasing`](crate::RenderData::antialiasing)
/// names it. The canvas, drawn after, isn't.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Antialiasing {
    /// The view drawn off by a cycle of 8 subpixel offsets, each pixel blended with its reprojected
    /// history, clipped to its neighborhood. Edges average what's drawn across their pixels.
    ///
    /// - A still pixel, each offset drawing it as a cycle before, goes unclipped, so what's thinner
    ///   than a pixel averages rather than flickers.
    /// - A pixel whose history the clip keeps rejecting as the view moves keeps its luminance,
    ///   taking less of each frame.
    /// - What comes and goes faster than every 20 frames shows its mean.
    ///
    /// What's opaque draws its motion since the frame before: a world model's by its instance's
    /// last transform, a game's shader's as it returns it, the rest the camera's alone.
    #[default]
    Taa,
    /// Each pixel shows what's drawn at its center.
    None,
    /// SMAA 1x (`shaders/smaa.slang`): from this frame's colors alone, each pixel blended with its
    /// neighbor across an edge by how much of it lies past the edge its line stands for. Sharper
    /// than TAA, without ghosts, but what's thinner than a pixel still flickers.
    Smaa,
}

impl Antialiasing {
    /// Each, in the order of their indices.
    pub const ALL: [Self; 3] = [Self::Taa, Self::None, Self::Smaa];
}

/// How many Halton offsets the view cycles through: `OFFSETS` in `shaders/taa.slang`, whose two
/// images of the offsets' draws hold four each.
const OFFSETS: u64 = 8;

/// Frame targets a [`History`] takes, from [`TaaPass::plan`]'s `first`.
pub(crate) const TARGETS: usize = 6;

/// The `index`th base-`base` Halton number: its digits mirrored about the point.
fn halton(mut index: u64, base: u64) -> f32 {
    let (mut fraction, mut result) = (1.0, 0.0);
    while index > 0 {
        fraction /= base as f32;
        result += fraction * (index % base) as f32;
        index /= base;
    }
    result
}

/// The `resolved`th resolving frame's offset in pixels, -0.5 to 0.5 each way: Halton bases 2 and 3
/// from index 1, cycling through [`OFFSETS`].
fn offset(resolved: u64) -> Vec2 {
    let index = resolved % OFFSETS + 1;
    Vec2::new(halton(index, 2), halton(index, 3)) - 0.5
}

/// Moves clip space by `offset` in normalized coordinates at any depth: X and Y by the offset times
/// W.
pub(crate) fn jitter(offset: Vec2) -> Mat4 {
    Mat4::from_cols(
        Vec4::X,
        Vec4::Y,
        Vec4::Z,
        Vec4::new(offset.x, offset.y, 0.0, 1.0),
    )
}

/// `offset`, in pixels of a view `size` pixels, in normalized coordinates, two units each way.
pub(crate) fn normalized(offset: Vec2, size: [u32; 2]) -> Vec2 {
    let [width, height] = size.map(|side| side as f32);
    offset * 2.0 / Vec2::new(width, height)
}

/// `Resolve` in `shaders/taa.slang`, field for field.
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
    held: u32,
    writes_resolved: u32,
    history_scale: f32,
    seen_first: u32,
    seen_second: u32,
    phase: u32,
    flicker: u32,
    flickers: u32,
    unused: [u32; 2],
}

/// `Push` in `shaders/taa.slang`.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct ResolvePush {
    resolve: u64,
}

/// The last resolving frame: its unjittered world-to-clip matrix, its exposure, and how many frames
/// in a row its resolve holds, its own included.
#[derive(Clone, Copy)]
struct Resolved {
    frame: u64,
    size: [u32; 2],
    clip_from_world: Mat4,
    exposure: f32,
    held: u32,
}

/// The resolve, the count of resolving frames (which the offsets follow), and the last one.
pub(crate) struct TaaPass {
    resolve: ComputePipeline,
    resolved: u64,
    last: Option<Resolved>,
}

/// The images the view resolves into and against, made again as its size changes, as the
/// [module](self) says. Each frame writes the colors and flicker test's the frame before didn't,
/// and the offsets' draws in place. Frames take all six as targets, apart from [`TaaPass`], while
/// they record the resolve.
#[derive(Default)]
pub(crate) struct History {
    images: Option<[StorageImage; TARGETS]>,
}

impl History {
    /// Makes its images again at `size`, if they're another size or none yet. The frame then
    /// resolves against none.
    pub(crate) fn prepare(&mut self, device: &Arc<Device>, size: [u32; 2]) -> Result<()> {
        if (self.images.as_ref()).is_some_and(|images| images[0].size() == size) {
            return Ok(());
        }
        // Those there retire until the GPU is done with them.
        self.images = None;
        let image = |name| StorageImage::new(device, size, 1, false, name);
        self.images = Some([
            image("taa history 0")?,
            image("taa history 1")?,
            image("taa seen 0")?,
            image("taa seen 1")?,
            image("taa flicker 0")?,
            image("taa flicker 1")?,
        ]);
        Ok(())
    }

    /// Its images as a frame's targets, at the indices [`TaaPass::plan`] gives them: none before
    /// [`prepare`](Self::prepare).
    pub(crate) fn targets(&mut self) -> impl Iterator<Item = Target<'_>> {
        (self.images.iter_mut()).flat_map(|images| images.iter_mut().map(Target::Storage))
    }
}

/// A frame's resolve in its graph. `history` and `flicker`, the frame before's, are there if its
/// resolve can be taken. `resolved` holds the view's resolved colors when written apart, for the
/// canvas to draw over.
pub(crate) struct Plan {
    pass: PassId,
    size: [u32; 2],
    drawn: [ImageId; 3],
    target: ImageId,
    history: Option<ImageId>,
    seen: [ImageId; 2],
    flickers: ImageId,
    flicker: Option<ImageId>,
    resolved: Option<ImageId>,
}

impl Plan {
    /// What the view shows: `resolved` if written, else `target`.
    pub(crate) fn shown(&self) -> ImageId {
        self.resolved.unwrap_or(self.target)
    }
}

/// How a frame drew its view: world to clip without and with its offset, the offset in normalized
/// coordinates, and the exposure lit shading multiplied light by.
#[derive(Clone, Copy)]
pub(crate) struct Drawn {
    pub(crate) unjittered: Mat4,
    pub(crate) clip_from_world: Mat4,
    pub(crate) jitter: Vec2,
    pub(crate) exposure: f32,
}

impl TaaPass {
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
        Ok(Self {
            resolve: ComputePipeline::new(device, "taa resolve", SPIRV, c"resolve_main")?,
            resolved: 0,
            last: None,
        })
    }

    /// The next resolving frame's offset in pixels, by [`offset`]. It counts resolving frames, not
    /// frame numbers, so a run's offsets follow from what it antialiased alone.
    pub(crate) fn offset(&self) -> Vec2 {
        offset(self.resolved)
    }

    /// Which of the [`OFFSETS`] the next resolving frame draws at: [`offset`](Self::offset)'s.
    pub(crate) fn phase(&self) -> u32 {
        (self.resolved % OFFSETS) as u32
    }

    /// Declares in `graph` the resolve of frame `frame`'s `drawn` colors, depth and motion, `size`
    /// pixels. A [`History`]'s targets start at `first`, in pairs: colors, offsets' draws, flicker
    /// test's.
    ///
    /// - Colors and flicker test: into the one the frame before didn't write, against the other if
    ///   that frame resolved at this size.
    /// - Offsets' draws: in place.
    /// - If `apart`, into the view's resolved colors too, made in `graph`.
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
        let seen = [graph.target(first + 2), graph.target(first + 3)];
        let states = [graph.target(first + 4), graph.target(first + 5)];
        let (into, against) = ((frame % 2) as usize, ((frame + 1) % 2) as usize);
        let (target, flickers) = (images[into], states[into]);
        let taken = (self.last).filter(|last| last.frame + 1 == frame && last.size == size);
        let history = taken.map(|_| images[against]);
        let flicker = taken.map(|_| states[against]);
        let resolved = apart.then(|| graph.hdr("view resolved", size));
        let pass = graph.compute(label!("taa resolve"));
        let pass = drawn
            .into_iter()
            .chain(history)
            .chain(flicker)
            .fold(pass, |pass, image| pass.sample(image));
        let pass = [target, flickers]
            .into_iter()
            .chain(resolved)
            .fold(pass, |pass, image| pass.overwrite(image));
        let pass = seen.into_iter().fold(pass, |pass, image| pass.write(image));
        Plan {
            pass: pass.add(),
            size,
            drawn,
            target,
            history,
            seen,
            flickers,
            flicker,
            resolved,
        }
    }

    /// Records `plan`'s resolve against the frame before's, and keeps this frame's view for the
    /// next.
    pub(crate) fn resolve(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &Plan,
        drawn: Drawn,
    ) -> Result<()> {
        let number = frame.number();
        let mut pass = frame.compute(plan.pass);
        let last = self.last.filter(|_| plan.history.is_some());
        let previous = last.map_or(drawn.unjittered, |last| last.clip_from_world);
        // The history at this frame's exposure, as an adapting one changes it.
        let history_scale = last.map_or(1.0, |last| rescale(drawn.exposure, last.exposure));
        let held = last.map_or(0, |last| last.held);
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
            held,
            writes_resolved: u32::from(plan.resolved.is_some()),
            history_scale,
            seen_first: pass.storage(plan.seen[0], 0),
            seen_second: pass.storage(plan.seen[1], 0),
            phase: self.phase(),
            flicker: plan.flicker.map_or(0, |flicker| pass.sampled(flicker)),
            flickers: pass.storage(plan.flickers, 0),
            unused: [0; 2],
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
            exposure: drawn.exposure,
            held: held.saturating_add(1),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offsets are the first eight Halton numbers of bases 2 and 3, within half a pixel each
    /// way, averaging to the pixel's center.
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

    /// A point moves by the offset in normalized coordinates at any depth, with perspective or
    /// without.
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
