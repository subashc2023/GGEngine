//! A camera's lens and film over the game's view: chromatic aberration, a vignette and film grain,
//! in a pass of their own before its canvas, so that a game's UI shows none of them, from the view's
//! colors into an image of its own, as `shaders/lens.slang` says. It isn't part of the tonemap
//! pass, which samples the view with its canvas drawn over it.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, PassId};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lens.spv"));

/// How many pixels across and down a workgroup takes: `GROUP` in `shaders/lens.slang`.
const GROUP: u32 = 8;

/// How far red and blue part from green at the view's edges, as a share of the way from its middle,
/// at a chromatic aberration of 1.
const ABERRATION: f32 = 0.01;

/// What a camera's lens and film do to the game's view, as
/// [`RenderData::lens`](crate::RenderData::lens) gives it, each from 0, for none, which
/// [`Lens::NONE`], the default, has of each, and costs nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    /// How much the view darkens toward its corners, by the cos⁴ law of a lens's light off its
    /// axis: the tangent of the angle off the axis its corners stand at, so that 1 leaves them a
    /// quarter as bright, and 0.5 about two thirds.
    pub vignette: f32,
    /// How far red and blue part from green toward the view's edges, red outward and blue inward,
    /// as a lens bends each color its own way: 1 parts them by a hundredth of the way from the
    /// view's middle, 7 pixels at the corners of a view 1,280 by 720.
    pub chromatic_aberration: f32,
    /// How much film grain scales each pixel's light by, either way, as a share, at most: triangular
    /// noise, alike in every channel, that changes each frame, by the frame's number, so that two
    /// captures of a paused game differ where there's grain. 0.1 is subtle.
    pub grain: f32,
}

impl Lens {
    /// None of them.
    pub const NONE: Self = Self {
        vignette: 0.0,
        chromatic_aberration: 0.0,
        grain: 0.0,
    };

    /// Whether it does anything to the view: any of its amounts is more than 0.
    pub(crate) fn shows(&self) -> bool {
        [self.vignette, self.chromatic_aberration, self.grain]
            .into_iter()
            .any(|value| amount(value) > 0.0)
    }
}

/// `value` as an amount the pass takes: none for one that isn't positive and finite.
fn amount(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

impl Default for Lens {
    fn default() -> Self {
        Self::NONE
    }
}

/// What the pass pushes: `Push` in `shaders/lens.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct LensPush {
    size: [u32; 2],
    source: u32,
    target: u32,
    aberration: f32,
    vignette: f32,
    grain: f32,
    frame: u32,
}

/// What lays a lens's effects over the game's view.
pub(crate) struct LensPass {
    pipeline: ComputePipeline,
}

/// The lens's pass in a frame's graph: the pass, what it does, the view's size, its colors, which
/// it samples, and those it writes, which the view shows after it.
pub(crate) struct Plan {
    pass: PassId,
    lens: Lens,
    size: [u32; 2],
    source: ImageId,
    lensed: ImageId,
}

impl Plan {
    /// What the view shows, through the lens.
    pub(crate) fn lensed(&self) -> ImageId {
        self.lensed
    }
}

impl LensPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        Ok(Self {
            pipeline: ComputePipeline::new(device, "lens", SPIRV, c"lens_main")?,
        })
    }

    /// Declares in `graph` the pass that lays `lens` over the view's `colors`, of `size` pixels,
    /// into colors of the view's own, made in `graph`.
    pub(crate) fn plan(graph: &mut Graph, lens: Lens, colors: ImageId, size: [u32; 2]) -> Plan {
        let lensed = graph.hdr("view lensed", size);
        let pass = graph.compute(label!("lens"));
        Plan {
            pass: pass.sample(colors).overwrite(lensed).add(),
            lens,
            size,
            source: colors,
            lensed,
        }
    }

    /// Records `plan`'s pass, its grain of the frame's number.
    pub(crate) fn record(&self, frame: &mut Frame<'_>, plan: &Plan) {
        // The grain's noise repeats every 2³² frames.
        let number = frame.number() as u32;
        let mut pass = frame.compute(plan.pass);
        let push = LensPush {
            size: plan.size,
            source: pass.sampled(plan.source),
            target: pass.storage(plan.lensed, 0),
            aberration: amount(plan.lens.chromatic_aberration) * ABERRATION,
            vignette: amount(plan.lens.vignette),
            grain: amount(plan.lens.grain),
            frame: number,
        };
        pass.set_pipeline(&self.pipeline);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = plan.size.map(|side| side.div_ceil(GROUP));
        pass.dispatch([across, down, 1]);
    }
}
