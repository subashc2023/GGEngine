//! The game's view, which the meshes draw into, its outline of a set of IDs, and its place in the
//! frame.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::Vec4;
use gg_core::{Result, label};
use gg_gpu::{
    Blend, ColorImage, ComputePipeline, Device, Format, Frame, Graph, GraphicsDesc, ImageId, Pass,
    PassId, Pipeline,
};

use crate::Viewport;

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/view.spv"));
static OUTLINE_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/outline.spv"));
static DISTANCES_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/outline_distances.spv"));

/// The widest an [`Outline`]'s band is, in pixels; the glow reaches as far again. Each pixel looks
/// `2 * MAX_OUTLINE + 1` pixels along its row and column, so the cost grows with the width, not its
/// square. The distances' shader holds [`MAX_REACH`] pixels of a row in groupshared memory.
const MAX_OUTLINE: f32 = 8.0;

/// The most a pixel looks along a row for the outline's IDs, for a band of [`MAX_OUTLINE`]:
/// `MAX_REACH` in `shaders/outline_distances.slang`.
const MAX_REACH: u32 = 17;

/// A workgroup of the distances' pass, in pixels: `GROUP_X` and `GROUP_Y` in
/// `shaders/outline_distances.slang`.
const GROUP_X: u32 = 64;
const GROUP_Y: u32 = 4;

/// Shows the game's view where the main window's UI paints it, as an egui `PaintCallback`: at the
/// callback's rect's top left, the view's size, clipped as the UI clips the callback, in the UI's
/// order. So the view can be a panel among the UI's, under some and over others, rather than at
/// [`RenderData::viewport`]'s origin under the whole UI.
///
/// [`RenderData::viewport`]: crate::RenderData::viewport
#[derive(Debug)]
pub struct ViewCallback;

/// An outline round what the game's view shows of a set of IDs' draws, such as an entity's and its
/// descendants', drawn into the view so it shows wherever the view does, and as it's read back. It
/// rings the shape their pixels on top make together, inside it too where another ID's draw covers
/// part, but not what's hidden. A solid band `width` pixels past their edge, its outer edge
/// smoothed, then a glow fading over as far again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline<'a> {
    /// Sorted, without 0 (draws of no entity) or repeats. None outlines nothing. The renderer
    /// searches them as they are and checks them only in debug builds.
    pub ids: &'a [u64],
    /// The band's color, in linear RGBA, alpha not premultiplied. The glow's is the same at up to
    /// half the alpha.
    pub color: Vec4,
    /// The band's width, in the view's pixels: at most 8, wider drawing 8. 0 or less, or NaN,
    /// outlines nothing. The glow reaches as far again.
    pub width: f32,
}

impl Outline<'_> {
    /// Whether it has IDs and a width.
    pub(crate) fn shows(&self) -> bool {
        !self.ids.is_empty() && self.width > 0.0
    }
}

/// Draws the view's image into the frame, each of its pixels onto one of the frame's.
pub(crate) struct ViewPass {
    pipeline: Pipeline,
}

impl ViewPass {
    pub(crate) fn new(device: &Arc<Device>, format: Format) -> Result<Self> {
        let pipeline = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: "view",
                spirv: SPIRV,
                vertex: c"vertex_main",
                mesh: None,
                fragment: Some(c"fragment_main"),
                format,
                blend: Blend::Opaque,
                depth_write: false,
                ids_write: false,
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        Ok(Self { pipeline })
    }

    /// Draws the view's image, whose index [`Pass::sampled`] gave this pass, into `viewport` of the
    /// pass's target, at the image's size, within `clip` of the target if given.
    ///
    /// # Panics
    ///
    /// - If the pass's target is of another format than the pass's.
    /// - If `viewport` has no area.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        viewport: Viewport,
        image: u32,
        clip: Option<Viewport>,
    ) {
        pass.set_viewport(viewport.origin, viewport.size);
        // After the viewport, which sets the scissor to itself.
        if let Some(clip) = clip {
            pass.set_scissor(clip.origin, clip.size);
        }
        pass.set_pipeline(&self.pipeline);
        let push = ViewPush {
            origin: viewport.origin,
            image,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw(3, 1);
    }
}

/// Draws an outline into the view's image, of [`ColorImage::FORMAT`], from the IDs its draws wrote
/// beside it: a pass finds how far along its row each pixel is from the nearest of the outline's
/// IDs, then a pass draws.
pub(crate) struct OutlinePass {
    distances: ComputePipeline,
    pipeline: Pipeline,
}

/// The outline's two passes in a frame's graph, the view's size, and the images they use.
pub(crate) struct OutlinePlan {
    find: PassId,
    draw: PassId,
    size: [u32; 2],
    ids: ImageId,
    distances: ImageId,
}

impl OutlinePass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let distances = ComputePipeline::new(
            device,
            "outline distances",
            DISTANCES_SPIRV,
            c"distances_main",
        )?;
        let pipeline = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: "outline",
                spirv: OUTLINE_SPIRV,
                vertex: c"vertex_main",
                mesh: None,
                fragment: Some(c"fragment_main"),
                format: ColorImage::FORMAT,
                blend: Blend::Alpha,
                depth_write: false,
                ids_write: false,
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        Ok(Self {
            distances,
            pipeline,
        })
    }

    /// Declares the passes outlining the view, `size` pixels, in its `color`, from its `ids`, over
    /// the view's tonemapping pass.
    pub(crate) fn plan(
        graph: &mut Graph,
        color: ImageId,
        ids: ImageId,
        size: [u32; 2],
    ) -> OutlinePlan {
        let distances = graph.scalar("outline distances", size);
        let find = (graph.compute(label!("outline distances")))
            .sample(ids)
            .overwrite(distances)
            .add();
        let draw = (graph.render(label!("outline")))
            .keep(color)
            .sample(distances)
            .add();
        OutlinePlan {
            find,
            draw,
            size,
            ids,
            distances,
        }
    }

    /// Records `plan`'s passes, uploading the outline's `ids` for the first.
    pub(crate) fn record(
        &self,
        frame: &mut Frame<'_>,
        plan: &OutlinePlan,
        outline: Outline<'_>,
    ) -> Result<()> {
        debug_assert!(
            outline.ids.first() != Some(&0) && outline.ids.is_sorted_by(|a, b| a < b),
            "an outline's IDs are sorted, without 0 or repeats"
        );
        let width = outline.width.min(MAX_OUTLINE);
        let reach = reach(width);
        let mut pass = frame.compute(plan.find);
        let push = DistancePush {
            list: pass.upload(bytemuck::cast_slice(outline.ids))?.address(),
            size: plan.size,
            image: pass.sampled(plan.ids),
            target: pass.storage(plan.distances, 0),
            count: outline.ids.len() as u32,
            reach,
        };
        pass.set_pipeline(&self.distances);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = plan.size;
        pass.dispatch([across.div_ceil(GROUP_X), down.div_ceil(GROUP_Y), 1]);
        drop(pass);
        let mut pass = (frame.render(plan.draw)).expect("the graph's own images are always there");
        pass.set_pipeline(&self.pipeline);
        let push = OutlinePush {
            color: outline.color.to_array(),
            image: pass.sampled(plan.distances),
            width,
            reach,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw(3, 1);
        Ok(())
    }
}

/// How far along a row the outline looks for its IDs, in pixels, for a band `width` wide: as far as
/// its glow reaches, and one more, so the distances' end isn't an edge of the outline.
fn reach(width: f32) -> u32 {
    let reach = (2.0 * width).ceil() as u32 + 1;
    debug_assert!(
        reach <= MAX_REACH,
        "a band of {width} pixels reaches {reach}"
    );
    reach
}

/// What the view pass pushes: `Push` in `shaders/view.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct ViewPush {
    /// The view's top left in the target, in pixels.
    origin: [u32; 2],
    /// The view's image's index in the bindless set.
    image: u32,
}

/// What the outline's first pass pushes: `Push` in `shaders/outline_distances.slang`, field for
/// field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct DistancePush {
    /// The IDs' address, each uploaded as little-endian halves, low first, as the view's IDs hold
    /// them.
    list: u64,
    size: [u32; 2],
    /// The view's IDs' index among the bindless set's textures, and the distances' among its
    /// storage images.
    image: u32,
    target: u32,
    count: u32,
    reach: u32,
}

/// What the outline's second pass pushes: `Push` in `shaders/outline.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct OutlinePush {
    /// Linear RGBA, alpha not premultiplied.
    color: [f32; 4],
    /// The distances' index in the bindless set.
    image: u32,
    /// The band's width in pixels.
    width: f32,
    reach: u32,
}
