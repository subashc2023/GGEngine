//! The view rendered below its size and upscaled to it (`shaders/upscale.slang`), as
//! [`RenderData::render_scale`](crate::RenderData::render_scale) asks. Everything before the canvas
//! renders at the scaled size; the canvas, tonemap and outline draw at the view's own.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::Result;
use gg_gpu::{Blend, Device, Format, GraphicsDesc, Pass, Pipeline};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/upscale.spv"));

/// The least render scale: below, the view would blur past use.
pub const MIN_RENDER_SCALE: f32 = 0.25;

/// The size a view of `size` pixels renders at by `scale`, as
/// [`RenderData::render_scale`](crate::RenderData::render_scale) says. A non-finite scale renders
/// at the view's size.
pub(crate) fn rendered(size: [u32; 2], scale: f32) -> [u32; 2] {
    let scale = if scale.is_finite() {
        scale.clamp(MIN_RENDER_SCALE, 1.0)
    } else {
        1.0
    };
    size.map(|side| ((side as f32 * scale).round() as u32).clamp(side.min(1), side))
}

/// `Push` in `shaders/upscale.slang`.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct UpscalePush {
    colors: u32,
    ids: u32,
    rendered: [u32; 2],
    size: [u32; 2],
}

pub(crate) struct UpscalePass {
    pipeline: Pipeline,
}

impl UpscalePass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: "upscale",
                spirv: SPIRV,
                vertex: c"vertex_main",
                mesh: None,
                fragment: Some(c"fragment_main"),
                format: Format::HDR.with_ids().with_motion().with_depth(),
                blend: Blend::Opaque,
                depth_write: false,
                ids_write: true,
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        Ok(Self { pipeline })
    }

    /// Draws the rendered `colors` and `ids`, sampled through those indices and `rendered` pixels,
    /// over the whole of `pass`'s targets, `size` pixels.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        [colors, ids]: [u32; 2],
        rendered: [u32; 2],
        size: [u32; 2],
    ) {
        pass.set_pipeline(&self.pipeline);
        let push = UpscalePush {
            colors,
            ids,
            rendered,
            size,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw(3, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sizes round to the nearest pixel, never below 1 or past the view's, and scales out of range
    /// clamp.
    #[test]
    fn rendered_sizes_round_and_clamp() {
        assert_eq!(rendered([3024, 1890], 0.5), [1512, 945]);
        assert_eq!(rendered([1280, 720], 1.0), [1280, 720]);
        assert_eq!(rendered([1280, 720], 2.0), [1280, 720]);
        assert_eq!(rendered([1280, 720], 0.0), [320, 180]);
        assert_eq!(rendered([1280, 720], f32::NAN), [1280, 720]);
        assert_eq!(rendered([1, 3], 0.25), [1, 1]);
        assert_eq!(rendered([0, 720], 0.5), [0, 360]);
    }
}
