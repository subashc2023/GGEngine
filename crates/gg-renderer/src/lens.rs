//! A camera's lens and film: chromatic aberration, a vignette and film grain
//! (`shaders/lens.slang`). It's a pass before the canvas, so a game's UI shows none of it; the
//! tonemap pass samples the view with the canvas over it.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, PassId};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lens.spv"));

/// Pixels across and down per workgroup: `GROUP` in `shaders/lens.slang`.
const GROUP: u32 = 8;

/// How far red and blue part from green at the view's edges, as a share of the way from its middle,
/// at an aberration of 1.
const ABERRATION: f32 = 0.01;

/// A lens's and film's effects, as [`RenderData::lens`](crate::RenderData::lens) gives them: each 0
/// for none. [`Lens::NONE`], the default, costs nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    /// How much the corners darken by the cos⁴ law: the tangent of their angle off the axis. 1
    /// leaves them a quarter as bright, 0.5 about two thirds.
    pub vignette: f32,
    /// How far red (outward) and blue (inward) part from green toward the edges. 1 parts them by a
    /// hundredth of the way from the middle: 7 pixels at the corners of 1,280 by 720.
    pub chromatic_aberration: f32,
    /// The most grain scales a pixel's light by, either way, as a share. The noise is triangular,
    /// alike in every channel, and changes each frame, so two captures of a paused game differ. 0.1
    /// is subtle.
    pub grain: f32,
}

impl Lens {
    pub const NONE: Self = Self {
        vignette: 0.0,
        chromatic_aberration: 0.0,
        grain: 0.0,
    };

    pub(crate) fn shows(&self) -> bool {
        [self.vignette, self.chromatic_aberration, self.grain]
            .into_iter()
            .any(|value| amount(value) > 0.0)
    }
}

/// `value`, or 0 if it isn't positive and finite.
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

/// `Push` in `shaders/lens.slang`, field for field.
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

pub(crate) struct LensPass {
    pipeline: ComputePipeline,
}

/// The lens's pass in a frame's graph, from `source` into `lensed`, which the view shows.
pub(crate) struct Plan {
    pass: PassId,
    lens: Lens,
    size: [u32; 2],
    source: ImageId,
    lensed: ImageId,
}

impl Plan {
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

    /// Declares in `graph` the pass laying `lens` over the view's `colors`, `size` pixels, into new
    /// colors of the graph's own.
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

    /// Records `plan`'s pass, its grain seeded by the frame's number.
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
