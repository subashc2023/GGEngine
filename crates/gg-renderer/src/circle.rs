//! Circles: discs and rings, drawn on the engine's quad by their own shader.

use bytemuck::NoUninit;
use gg_core::math::Vec4;

/// `shaders/circle.slang`, compiled by the build.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/circle.spv"));

/// A disc or ring filling a unit square centered on its origin in its XY plane, facing +Z.
///
/// As a sprite does. The draw's transform places, sizes and turns it; an uneven scale draws an
/// ellipse. It blends by straight alpha over what's drawn before it, testing depth without writing it. Its
/// edges are smoothed over a pixel at any size. Where it's fully transparent it draws nothing, so
/// picks there find what's behind it. `Circle::default()` is a white disc.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    /// Linear RGBA, alpha not premultiplied. White by default.
    pub color: Vec4,
    /// How far in from its edge it's filled, as a fraction of its radius. 1 (the default) or more
    /// fills a disc; less draws a ring, 0.1 a tenth of the radius thick.
    pub thickness: f32,
    /// How far in from its edges it fades, as a fraction of its radius, beyond the smoothed pixel.
    /// 0 (the default) is crisp.
    pub fade: f32,
}

impl Default for Circle {
    fn default() -> Self {
        Self {
            color: Vec4::ONE,
            thickness: 1.0,
            fade: 0.0,
        }
    }
}

impl Circle {
    pub(crate) fn params(&self) -> Params {
        Params {
            color: self.color.to_array(),
            thickness: self.thickness,
            fade: self.fade,
        }
    }
}

/// What `shaders/circle.slang` takes per circle, an instance of its draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    color: [f32; 4],
    thickness: f32,
    fade: f32,
}
