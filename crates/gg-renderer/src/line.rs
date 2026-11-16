//! Lines: straight lines a width in the view's pixels, drawn on the engine's quad by their own
//! shader.
//!
//! Quads rather than Vulkan's lines: those share one width per draw, and wide and smooth lines are
//! optional features. As quads, a run of lines of any widths is one instanced draw, and draws the
//! same on every device.

use bytemuck::NoUninit;
use gg_core::math::{Vec3, Vec4};

/// `shaders/line.slang`, compiled by the build.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/line.spv"));

/// A straight line from `from` to `to`, in the space the draw's transform takes to the world's.
/// It's `width` pixels wide at any distance, whatever the transform's scale. Its ends are squared
/// off half its width past its points, so lines meeting at a point leave no gap, though a
/// translucent color shows darker where they overlap. Its edges are smoothed over a pixel. It
/// blends by straight alpha over what's drawn before it, testing depth without writing it, and is
/// cut at the near plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub from: Vec3,
    pub to: Vec3,
    /// Linear RGBA, alpha not premultiplied.
    pub color: Vec4,
    /// In the view's pixels. Thinner than a pixel draws a pixel wide and fainter by as much, so it
    /// neither breaks up nor vanishes.
    pub width: f32,
}

impl Line {
    pub(crate) fn params(&self) -> Params {
        Params {
            from: self.from,
            to: self.to,
            color: self.color.to_array(),
            width: self.width,
        }
    }
}

/// What `shaders/line.slang` takes per line, an instance of its draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    from: Vec3,
    to: Vec3,
    color: [f32; 4],
    width: f32,
}
