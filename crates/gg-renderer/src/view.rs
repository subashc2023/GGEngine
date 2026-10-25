//! The game's view, which the meshes are drawn into, shown in the frame where the view is.

use std::sync::Arc;

use gg_core::Result;
use gg_gpu::{Blend, Device, Format, GraphicsDesc, Pass, Pipeline};

use crate::Viewport;

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/view.spv"));

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
                fragment: c"fragment_main",
                format,
                blend: Blend::Opaque,
                depth_write: false,
            },
        )?;
        Ok(Self { pipeline })
    }

    /// Draws the view's image into `viewport` of the pass's target, which is the image's size.
    /// `image` is the index [`Frame::begin_sampling`](gg_gpu::Frame::begin_sampling) gave it this
    /// frame.
    ///
    /// # Panics
    ///
    /// If the pass's target has another format than the pass's, or `viewport` has no area.
    pub(crate) fn draw(&self, pass: &mut Pass<'_, '_>, viewport: Viewport, image: u32) {
        pass.set_viewport(viewport.origin, viewport.size);
        pass.set_pipeline(&self.pipeline);
        // The shader's `Push`: the view's origin in the target, then the image's index.
        let [x, y] = viewport.origin;
        let mut push = [0u8; 12];
        push[0..4].copy_from_slice(&x.to_ne_bytes());
        push[4..8].copy_from_slice(&y.to_ne_bytes());
        push[8..12].copy_from_slice(&image.to_ne_bytes());
        pass.push_constants(&push);
        pass.draw(3, 1);
    }
}
