//! The game's view, which the meshes are drawn into, and the outline round what it shows of an ID
//! after them, shown in the frame where the view is.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::math::Vec4;
use gg_gpu::{Blend, ColorImage, Device, Format, GraphicsDesc, Pass, Pipeline};

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
/// into the view over them, so that wherever the view shows, under the UI, and as it's read back,
/// it shows outlined: round whatever shape their pixels make where they're on top, as the IDs the
/// view writes say, inside it too where a draw of another ID covers some of it, but not round
/// what's hidden.
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

/// Draws the view's image into the frame, each of its pixels onto one of the frame's.
pub(crate) struct ViewPass {
    pipeline: Pipeline,
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
                mesh: None,
                fragment: Some(c"fragment_main"),
                format,
                blend: Blend::Opaque,
                depth_write: false,
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        Ok(Self { pipeline })
    }

    /// Draws the view's image, whose index [`Pass::sampled`] gave this pass, into `viewport` of the
    /// pass's target, which is the image's size, within `clip` of the target if it's given.
    ///
    /// # Panics
    ///
    /// If the pass's target has another format than the pass's, or `viewport` has no area.
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

/// Draws an outline into the view's image, of [`ColorImage::FORMAT`], over its draws, from the IDs
/// they wrote beside it.
pub(crate) struct OutlinePass {
    pipeline: Pipeline,
}

impl OutlinePass {
    /// A pass that draws into the view's image.
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
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
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        Ok(Self { pipeline })
    }

    /// Draws `outline` over the whole of the pass's target, the view's image, from the view's IDs,
    /// whose index [`Pass::sampled`] gave this pass as `ids`.
    ///
    /// # Panics
    ///
    /// If the pass's target isn't of [`ColorImage::FORMAT`], without IDs or a depth image.
    pub(crate) fn draw(&self, pass: &mut Pass<'_, '_>, ids: u32, outline: Outline) {
        pass.set_pipeline(&self.pipeline);
        let push = OutlinePush {
            color: outline.color.to_array(),
            id: halves(outline.id),
            image: ids,
            width: outline.width.min(MAX_OUTLINE),
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw(3, 1);
    }
}

/// What the view pass pushes before it draws the view's image: `Push` in `shaders/view.slang`,
/// field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct ViewPush {
    /// Where the view's top left is in the target, in pixels.
    origin: [u32; 2],
    /// The index of the view's image in the bindless set.
    image: u32,
}

/// What the outline pass pushes before it draws an outline: `Push` in `shaders/outline.slang`,
/// field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct OutlinePush {
    /// Linear RGBA, alpha not multiplied in.
    color: [f32; 4],
    /// The ID's low and high halves.
    id: [u32; 2],
    /// The index of the view's IDs in the bindless set.
    image: u32,
    /// How far out it reaches from the ID's pixels, in pixels.
    width: f32,
}
