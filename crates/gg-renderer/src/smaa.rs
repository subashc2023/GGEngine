//! SMAA 1x (`shaders/smaa.slang`), in TAA's place before bloom, the grade and the canvas. It finds
//! edges, measures their lines through `lookup`'s textures, and blends each pixel with its neighbor
//! across the edge by the share the line cuts off.
//!
//! It blends colors as the tonemapper will show them, not weighed by 1 / (1 + luminance) as TAA's
//! resolve does. That would darken every edge's blend and squash bright edges' contrast under the
//! threshold.

mod lookup;

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, PassId, Texture, TextureFormat};

use crate::{Grade, Tonemapper};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/smaa.spv"));

/// Pixels across and down per workgroup: `GROUP` in `shaders/smaa.slang`.
const GROUP: u32 = 8;

/// `Push` in `shaders/smaa.slang`, field for field. Each pass sets those it reads and writes.
#[repr(C)]
#[derive(Clone, Copy, Default, NoUninit)]
struct SmaaPush {
    size: [u32; 2],
    colors: u32,
    edges: u32,
    weights: u32,
    area: u32,
    search: u32,
    target: u32,
    curve: [f32; 4],
}

/// Where SMAA's view of light starts rolling off toward 1 under `tonemapper` (`seen` in
/// `shaders/smaa.slang`). PBR Neutral's 0.76 serves ACES and AgX too, which roll off sooner and
/// more gently. Under none, which clamps at 1, it's 1 − 1/256: all but the clamp, yet invertible.
fn shoulder(tonemapper: Tonemapper) -> f32 {
    match tonemapper {
        Tonemapper::None => 255.0 / 256.0,
        Tonemapper::PbrNeutral | Tonemapper::Aces | Tonemapper::Agx => 0.76,
    }
}

/// A pipeline per pass, and the lookup textures, which the first frame to record SMAA writes.
pub(crate) struct SmaaPass {
    edges: ComputePipeline,
    weights: ComputePipeline,
    blend: ComputePipeline,
    area: Texture,
    search: Texture,
    written: bool,
}

/// A frame's SMAA in its graph: its edges, weights and antialiased `target`, images of the graph's
/// own.
pub(crate) struct Plan {
    size: [u32; 2],
    colors: ImageId,
    edges: ImageId,
    weights: ImageId,
    target: ImageId,
    passes: [PassId; 3],
}

impl Plan {
    pub(crate) fn shown(&self) -> ImageId {
        self.target
    }
}

impl SmaaPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        let texture = |[width, height]: [u32; 2], name| {
            Texture::new(
                device,
                width,
                height,
                TextureFormat::Rgba8Unorm,
                false,
                name,
            )
        };
        Ok(Self {
            edges: pipeline("smaa edges", c"edges_main")?,
            weights: pipeline("smaa weights", c"weights_main")?,
            blend: pipeline("smaa blend", c"blend_main")?,
            area: texture(lookup::AREA_SIZE, "smaa area")?,
            search: texture(lookup::SEARCH_SIZE, "smaa search")?,
            written: false,
        })
    }

    /// Writes the lookup textures in `frame`, before its passes, unless a frame has. They take a
    /// millisecond or so to make.
    pub(crate) fn write_textures(&mut self, frame: &mut Frame<'_>) -> Result<()> {
        if self.written {
            return Ok(());
        }
        let area = lookup::area();
        frame.write_texture(&mut self.area, [0, 0], lookup::AREA_SIZE, &area)?;
        let search = lookup::search();
        frame.write_texture(&mut self.search, [0, 0], lookup::SEARCH_SIZE, &search)?;
        self.written = true;
        Ok(())
    }

    /// Declares in `graph` the SMAA of the view's HDR `colors`, `size` pixels, that an earlier pass
    /// wrote.
    pub(crate) fn plan(graph: &mut Graph, colors: ImageId, size: [u32; 2]) -> Plan {
        let edges = graph.hdr("smaa edges", size);
        let weights = graph.hdr("smaa weights", size);
        let target = graph.hdr("view antialiased", size);
        let find = graph.compute(label!("smaa edges"));
        let find = find.sample(colors).overwrite(edges).add();
        let weigh = graph.compute(label!("smaa weights"));
        let weigh = weigh.sample(edges).overwrite(weights).add();
        let blend = graph.compute(label!("smaa blend"));
        let blend = blend.sample(colors).sample(weights).overwrite(target).add();
        Plan {
            size,
            colors,
            edges,
            weights,
            target,
            passes: [find, weigh, blend],
        }
    }

    /// Records `plan`'s passes, seeing the view's light roughly as `grade`'s slope and `tonemapper`
    /// will show it, as `shaders/smaa.slang` says.
    ///
    /// # Panics
    ///
    /// - If no frame has written the lookup textures.
    pub(crate) fn record(
        &self,
        frame: &mut Frame<'_>,
        plan: &Plan,
        tonemapper: Tonemapper,
        grade: &Grade,
    ) {
        assert!(self.written, "a frame writes SMAA's lookup textures first");
        let [find, weigh, blend] = plan.passes;
        let [across, down] = plan.size.map(|side| side.div_ceil(GROUP));
        let size = plan.size;
        let curve = grade.slope().extend(shoulder(tonemapper)).to_array();
        let mut pass = frame.compute(find);
        let push = SmaaPush {
            size,
            colors: pass.sampled(plan.colors),
            target: pass.storage(plan.edges, 0),
            curve,
            ..SmaaPush::default()
        };
        pass.set_pipeline(&self.edges);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch([across, down, 1]);
        drop(pass);
        let mut pass = frame.compute(weigh);
        let push = SmaaPush {
            size,
            edges: pass.sampled(plan.edges),
            area: self.area.index(),
            search: self.search.index(),
            target: pass.storage(plan.weights, 0),
            ..SmaaPush::default()
        };
        pass.set_pipeline(&self.weights);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch([across, down, 1]);
        drop(pass);
        let mut pass = frame.compute(blend);
        let push = SmaaPush {
            size,
            colors: pass.sampled(plan.colors),
            weights: pass.sampled(plan.weights),
            target: pass.storage(plan.target, 0),
            curve,
            ..SmaaPush::default()
        };
        pass.set_pipeline(&self.blend);
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch([across, down, 1]);
    }
}
