//! The game's view, which the meshes are drawn into, shown in the frame where the view is, and
//! the outline round what it shows of an ID.

use std::sync::Arc;

use gg_core::Result;
use gg_core::math::Vec4;
use gg_gpu::{Blend, Device, Format, GraphicsDesc, Pass, Pipeline};

use crate::{Viewport, halves};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/view.spv"));
static OUTLINE_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/outline.spv"));

/// The widest an [`Outline`] draws, in pixels: each pixel of the view reads the IDs within its
/// width of it, so that its cost grows with the square of the width.
const MAX_OUTLINE: f32 = 8.0;

/// Shows the game's view where the main window's UI paints it, as the callback of an egui
/// `PaintCallback`: from the callback's rect's top left, the view's size, clipped as the UI clips
/// the callback, in the UI's order, rather than at [`RenderData::viewport`]'s origin under the
/// whole UI. So the view can be a panel among the UI's, under some and over others.
///
/// [`RenderData::viewport`]: crate::RenderData::viewport
#[derive(Debug)]
pub struct ViewCallback;

/// An outline round what the game's view shows of the draws of one ID, such as an entity's, drawn
/// over the view where the frame shows it, under the UI: round whatever shape their pixels make
/// where they're on top, as the IDs the view writes say, inside it too where a draw of another ID
/// covers some of it, but not round what's hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    /// The ID; 0, which draws of no entity have, outlines nothing.
    pub id: u64,
    /// Its color, in linear RGBA, alpha not multiplied in.
    pub color: Vec4,
    /// How far out it reaches from the ID's pixels, in the view's pixels, its outer edge smoothed:
    /// at most 8, a width past that drawing 8, and one of 0 or less, or NaN, outlining nothing.
    pub width: f32,
}

impl Outline {
    /// Whether it outlines anything: it has an ID and a width.
    pub(crate) fn shows(&self) -> bool {
        self.id != 0 && self.width > 0.0
    }
}

/// Where the frame shows the view from: its image's index this frame, and if it's outlined, the
/// index of its IDs this frame, with the outline.
#[derive(Clone, Copy)]
pub(crate) struct Shown {
    pub(crate) image: u32,
    pub(crate) outline: Option<(u32, Outline)>,
}

/// Draws the view's image into the frame, each of its pixels onto one of the frame's, and its
/// outline over it.
pub(crate) struct ViewPass {
    pipeline: Pipeline,
    outline: Pipeline,
}

impl ViewPass {
    /// A pass that draws into targets of `format`.
    pub(crate) fn new(device: &Arc<Device>, format: Format) -> Result<Self> {
        let pipeline = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: "view",
                spirv: SPIRV,
                vertex: c"vertex_main",
                fragment: c"fragment_main",
                format,
                blend: Blend::Opaque,
                depth_write: false,
            },
        )?;
        let outline = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: "outline",
                spirv: OUTLINE_SPIRV,
                vertex: c"vertex_main",
                fragment: c"fragment_main",
                format,
                blend: Blend::Alpha,
                depth_write: false,
            },
        )?;
        Ok(Self { pipeline, outline })
    }

    /// Draws the view's image into `viewport` of the pass's target, which is the image's size,
    /// within `clip` of the target if it's given, and its outline over it, if it has one. `shown`
    /// holds the indices [`Frame::begin_sampling`](gg_gpu::Frame::begin_sampling) and
    /// [`Frame::begin_sampling_ids`](gg_gpu::Frame::begin_sampling_ids) gave them this frame.
    ///
    /// # Panics
    ///
    /// If the pass's target has another format than the pass's, or `viewport` has no area.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        viewport: Viewport,
        shown: Shown,
        clip: Option<Viewport>,
    ) {
        pass.set_viewport(viewport.origin, viewport.size);
        // After the viewport, which sets the scissor to itself.
        if let Some(clip) = clip {
            pass.set_scissor(clip.origin, clip.size);
        }
        pass.set_pipeline(&self.pipeline);
        // The shader's `Push`: the view's origin in the target, then the image's index.
        let [x, y] = viewport.origin;
        let mut push = [0u8; 12];
        push[0..4].copy_from_slice(&x.to_ne_bytes());
        push[4..8].copy_from_slice(&y.to_ne_bytes());
        push[8..12].copy_from_slice(&shown.image.to_ne_bytes());
        pass.push_constants(&push);
        pass.draw(3, 1);
        let Some((ids, outline)) = shown.outline else {
            return;
        };
        pass.set_pipeline(&self.outline);
        // The shader's `Push`: the color, the view's origin in the target, the ID's halves, the
        // IDs' index, then the width.
        let mut push = [0u8; 40];
        for (i, channel) in outline.color.to_array().into_iter().enumerate() {
            push[4 * i..4 * i + 4].copy_from_slice(&channel.to_ne_bytes());
        }
        push[16..20].copy_from_slice(&x.to_ne_bytes());
        push[20..24].copy_from_slice(&y.to_ne_bytes());
        let [low, high] = halves(outline.id);
        push[24..28].copy_from_slice(&low.to_ne_bytes());
        push[28..32].copy_from_slice(&high.to_ne_bytes());
        push[32..36].copy_from_slice(&ids.to_ne_bytes());
        push[36..40].copy_from_slice(&outline.width.min(MAX_OUTLINE).to_ne_bytes());
        pass.push_constants(&push);
        pass.draw(3, 1);
    }
}
