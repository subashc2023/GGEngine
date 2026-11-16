//! Bloom: light spreading round what's bright, as through a lens. A chain downsamples the view's
//! colors and upsamples back (`shaders/bloom.slang`). The last pass blends it in before the canvas
//! and grades as [`Grade`] says, alone where nothing blooms.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, PassId};

use crate::grade::{Grade, GradeData};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bloom.spv"));

/// Texels across and down per workgroup: `GROUP` in `shaders/bloom.slang`.
const GROUP: u32 = 8;

/// The most levels: a view's of 4,096 pixels or more on its shorter side.
const MOST: usize = 8;

/// How far each upsample lerps a level toward the one below. The first level ends up 1 − s of
/// itself, s(1 − s) of the second, and so on, the last taking the rest: about a sixth at six
/// levels. Wide, faint light, as a lens spreads it.
const SHARE: f32 = 0.7;

/// Enough levels to halve the shorter side to 16 to 32 texels, up to [`MOST`], so the last spans
/// the same share of any view. None under 32 pixels.
fn levels(size: [u32; 2]) -> usize {
    let shorter = size[0].min(size[1]);
    (shorter.max(1).ilog2() as usize)
        .saturating_sub(4)
        .min(MOST)
}

/// Whether a view `size` pixels blooms by `intensity`: above 0, and with [`levels`].
pub(crate) fn blooms(intensity: f32, size: [u32; 2]) -> bool {
    intensity > 0.0 && levels(size) > 0
}

/// Each level's size: half the one before, rounded up.
fn sizes(size: [u32; 2]) -> impl Iterator<Item = [u32; 2]> {
    let half = |size: &[u32; 2]| Some(size.map(|side| side.div_ceil(2)));
    std::iter::successors(half(&size), half).take(levels(size))
}

/// `Push` in `shaders/bloom.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct BloomPush {
    size: [u32; 2],
    source_size: [u32; 2],
    source: u32,
    target: u32,
    share: f32,
    /// Of the last pass, which of [`BLOOMS`](Self::BLOOMS) and [`GRADES`](Self::GRADES) it does.
    does: u32,
    grade: GradeData,
}

impl BloomPush {
    const BLOOMS: u32 = 1;
    const GRADES: u32 = 2;
}

pub(crate) struct BloomPass {
    first: ComputePipeline,
    down: ComputePipeline,
    up: ComputePipeline,
    composite: ComputePipeline,
}

/// A frame's bloom and grade in its graph. The chain has [`levels`] levels, or none if the view
/// doesn't bloom.
pub(crate) struct Plan {
    colors: ImageId,
    size: [u32; 2],
    levels: [Option<Level>; MOST],
    composite: PassId,
}

/// A level: its image and the passes into it. The last has no upsample.
#[derive(Clone, Copy)]
struct Level {
    image: ImageId,
    size: [u32; 2],
    down: PassId,
    up: Option<PassId>,
}

impl BloomPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        Ok(Self {
            first: pipeline("bloom first", c"first_main")?,
            down: pipeline("bloom down", c"down_main")?,
            up: pipeline("bloom up", c"up_main")?,
            composite: pipeline("bloom composite", c"composite_main")?,
        })
    }

    /// Declares in `graph` the bloom, if it `blooms`, and the grade, if it `grades`, of the view's
    /// HDR `colors`, `size` pixels, in place.
    ///
    /// # Panics
    ///
    /// - If it blooms a view too small to, as [`blooms`] says.
    pub(crate) fn plan(
        graph: &mut Graph,
        colors: ImageId,
        size: [u32; 2],
        blooms: bool,
        grades: bool,
    ) -> Plan {
        assert!(
            !blooms || levels(size) > 0,
            "a view of {size:?} is too small to bloom"
        );
        let names = [
            "bloom 1", "bloom 2", "bloom 3", "bloom 4", "bloom 5", "bloom 6", "bloom 7", "bloom 8",
        ];
        let downs = [
            label!("bloom down 1"),
            label!("bloom down 2"),
            label!("bloom down 3"),
            label!("bloom down 4"),
            label!("bloom down 5"),
            label!("bloom down 6"),
            label!("bloom down 7"),
            label!("bloom down 8"),
        ];
        let ups = [
            label!("bloom up 1"),
            label!("bloom up 2"),
            label!("bloom up 3"),
            label!("bloom up 4"),
            label!("bloom up 5"),
            label!("bloom up 6"),
            label!("bloom up 7"),
        ];
        // Each level from the one before, the first from the view's colors.
        let mut chain = [None; MOST];
        let mut before = colors;
        let count = if blooms { levels(size) } else { 0 };
        for (i, size) in sizes(size).take(count).enumerate() {
            let image = graph.hdr(names[i], size);
            let down = (graph.compute(downs[i]).sample(before).overwrite(image)).add();
            chain[i] = Some(Level {
                image,
                size,
                down,
                up: None,
            });
            before = image;
        }
        // From the last up, each level toward the one below it.
        for i in (0..count.saturating_sub(1)).rev() {
            let below = chain[i + 1].expect("a level below").image;
            let level = chain[i].as_mut().expect("a level");
            level.up = Some((graph.compute(ups[i]).sample(below).write(level.image)).add());
        }
        let label = match (blooms, grades) {
            (true, false) => label!("bloom"),
            (true, true) => label!("bloom and grade"),
            (false, _) => label!("grade"),
        };
        let composite = graph.compute(label);
        let composite = match chain[0] {
            Some(first) => composite.sample(first.image),
            None => composite,
        };
        Plan {
            colors,
            size,
            levels: chain,
            composite: composite.write(colors).add(),
        }
    }

    /// Records `plan`: blends the first level in by `intensity` (0 none, 1 the first level alone),
    /// if it blooms, and grades by `grade` unless that's the identity.
    pub(crate) fn record(&self, frame: &mut Frame<'_>, plan: &Plan, intensity: f32, grade: &Grade) {
        // A chain pass: `pipeline` from `source` to `target`, each an image and its size.
        let dispatch = |frame: &mut Frame<'_>, pass, pipeline, source, target, share| {
            let (source, source_size): (ImageId, [u32; 2]) = source;
            let (target, size): (ImageId, [u32; 2]) = target;
            let mut pass = frame.compute(pass);
            let push = BloomPush {
                size,
                source_size,
                source: pass.sampled(source),
                target: pass.storage(target, 0),
                share,
                does: 0,
                grade: GradeData::NONE,
            };
            pass.set_pipeline(pipeline);
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch([size[0].div_ceil(GROUP), size[1].div_ceil(GROUP), 1]);
        };
        let view = (plan.colors, plan.size);
        let levels = plan.levels.iter().flatten();
        let mut before = view;
        for (i, level) in levels.clone().enumerate() {
            let pipeline = if i == 0 { &self.first } else { &self.down };
            let target = (level.image, level.size);
            dispatch(frame, level.down, pipeline, before, target, 0.0);
            before = target;
        }
        // `before` starts as the last level, which the one above it reads first.
        for level in levels.rev().skip(1) {
            let target = (level.image, level.size);
            let up = level.up.expect("each level but the last upsamples");
            dispatch(frame, up, &self.up, before, target, SHARE);
            before = target;
        }
        let first = plan.levels[0];
        let grades = !grade.is_identity();
        let mut pass = frame.compute(plan.composite);
        let does =
            (first.map_or(0, |_| BloomPush::BLOOMS)) | if grades { BloomPush::GRADES } else { 0 };
        let push = BloomPush {
            size: plan.size,
            source_size: first.map_or([0; 2], |first| first.size),
            source: first.map_or(0, |first| pass.sampled(first.image)),
            target: pass.storage(plan.colors, 0),
            share: intensity.clamp(0.0, 1.0),
            does,
            grade: if grades {
                grade.data()
            } else {
                GradeData::NONE
            },
        };
        pass.set_pipeline(&self.composite);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [width, height] = plan.size;
        pass.dispatch([width.div_ceil(GROUP), height.div_ceil(GROUP), 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 5 levels at 1,280 by 720, 6 at 1,920 by 1,080, none under 32 pixels, at most 8. Each is half
    /// the one before, rounded up.
    #[test]
    fn the_chain_halves_the_view_to_between_16_and_32_texels() {
        assert_eq!(levels([1280, 720]), 5);
        assert_eq!(levels([1920, 1080]), 6);
        assert_eq!(levels([1080, 1920]), 6);
        assert_eq!(levels([31, 400]), 0);
        assert_eq!(levels([32, 32]), 1);
        assert_eq!(levels([0, 0]), 0);
        assert_eq!(levels([1 << 20, 1 << 20]), MOST);
        let chain: Vec<[u32; 2]> = sizes([1920, 1080]).collect();
        let expected = [
            [960, 540],
            [480, 270],
            [240, 135],
            [120, 68],
            [60, 34],
            [30, 17],
        ];
        assert_eq!(chain, expected);
    }
}
