//! Circles: discs and rings of a color, which the engine draws on its quad with a shader of its
//! own.

use bytemuck::NoUninit;
use gg_core::math::Vec4;

/// The circle shader, which the build compiled from `shaders/circle.slang`.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/circle.spv"));

/// A disc or a ring of a color, on a square a unit across, centered on its origin in its XY plane
/// and facing +Z, as a sprite is, as large as fits in it: a unit across. The draw's transform
/// places, sizes and turns it in the world, a scale uneven across and up drawing an ellipse, and
/// it's drawn by straight alpha over what's drawn before it, but where an opaque mesh is nearer,
/// testing depth without writing it. Its edges are smoothed over a pixel however big it shows; and
/// where it's fully transparent, past its edge and inside a ring, it draws nothing, so that picks
/// there find what's behind it. `Circle::default()` is a white disc.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    /// In linear RGBA, alpha not multiplied in: white by default.
    pub color: Vec4,
    /// How far in from its edge it's filled, as a fraction of its radius: 1, the default, or more
    /// fills it all, a disc; less draws a ring that thick, 0.1 a tenth of its radius.
    pub thickness: f32,
    /// How far in from each of its edges it fades, as a fraction of its radius, beyond the pixel
    /// its edges are smoothed over: 0 by default, for crisp edges.
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
    /// What the circle shader takes for the circle, an instance of its draw.
    pub(crate) fn params(&self) -> Params {
        Params {
            color: self.color.to_array(),
            thickness: self.thickness,
            fade: self.fade,
        }
    }
}

/// What `shaders/circle.slang` takes for each circle, an instance of its draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    color: [f32; 4],
    thickness: f32,
    fade: f32,
}
