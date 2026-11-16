//! Glyphs: a font's letters, drawn from its distance field on the engine's quad by their own
//! shader, crisp at any size.

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec4};

use crate::Texture;

/// `shaders/text.slang`, compiled by the build.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/text.spv"));

/// A glyph of text, drawn from its font's multi-channel signed distance field: a
/// [data](crate::Texels::Data) texture whose red, green and blue hold distances to the glyph's
/// edges. Their median is the distance to the nearest edge, keeping corners sharp: 0.5 at the edge,
/// rising inside and falling outside.
///
/// A glyph is a unit square centered on its origin in its XY plane, facing +Z, as a sprite is. It
/// shows `rect` of the field (top left at its top left, +Y up), filled with `color` inside the
/// glyph. The draw's transform places, sizes and turns it. Its edge is smoothed over a pixel at any
/// size: sharp while the field's texels are no larger than pixels, and blurred where they're
/// larger, rather than aliased as a glyph drawn smaller than its field is. It blends by straight
/// alpha over what's drawn before it, testing depth without writing it. Where it's fully
/// transparent it draws nothing, so picks there find what's behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// Linear RGBA, alpha not premultiplied.
    pub color: Vec4,
    /// Sampled linearly, clamped to its edges.
    pub field: Texture,
    /// The part of `field` shown, top left then bottom right, in texture coordinates: (0, 0) at the
    /// field's top left, (1, 1) at its bottom right.
    pub rect: [Vec2; 2],
    /// How far the field's distances reach, in its texels: `-range / 2` at 0 to `range / 2` at 1.
    pub range: f32,
}

impl Glyph {
    pub(crate) fn params(&self) -> Params {
        Params {
            color: self.color.to_array(),
            rect: self.rect,
            field: self.field,
            range: self.range,
        }
    }
}

/// What `shaders/text.slang` takes per glyph, an instance of its draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    color: [f32; 4],
    rect: [Vec2; 2],
    field: Texture,
    range: f32,
}
