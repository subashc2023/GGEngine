//! Glyphs: a font's letters, which the engine draws from its distance field on its quad with a
//! shader of its own, crisp at any size.

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec4};

use crate::Texture;

/// The text shader, which the build compiled from `shaders/text.slang`.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/text.spv"));

/// A glyph of text, drawn from its font's field: a texture of [data](crate::Texels::Data) whose red,
/// green and blue hold distances to the glyph's edges, a multi-channel signed distance field, whose
/// median is the distance to the nearest edge but keeps the glyph's corners sharp, from 0.5 at its
/// edge up inside it and down outside it. A glyph is a square a unit across, as a sprite is,
/// centered on its origin in its XY plane and facing +Z, showing `rect` of the field, its top left
/// at its top left, +Y being up, filled with `color` where the field says it's inside the glyph.
/// The draw's transform places, sizes and turns it in the world. Its edge is smoothed over a pixel
/// however big it shows, sharp at any size the field's texels are no larger than pixels, and
/// blurred where they are, rather than aliased, as a glyph drawn smaller than its field is; and
/// it's drawn by straight alpha over what's drawn before it, but where an opaque mesh is nearer,
/// testing depth without writing it, and where it's fully transparent, between its strokes, it
/// draws nothing, so that picks there find what's behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// In linear RGBA, alpha not multiplied in.
    pub color: Vec4,
    /// The field it's drawn from, sampled linearly and clamped to its edges.
    pub field: Texture,
    /// The part of `field` it shows, its top left then its bottom right, in texture coordinates,
    /// from (0, 0) at the field's top left to (1, 1) at its bottom right.
    pub rect: [Vec2; 2],
    /// How far the field's distances reach, in its texels: from `-range / 2` at 0 to `range / 2`
    /// at 1.
    pub range: f32,
}

impl Glyph {
    /// What the text shader takes for the glyph, an instance of its draw.
    pub(crate) fn params(&self) -> Params {
        Params {
            color: self.color.to_array(),
            rect: self.rect,
            field: self.field,
            range: self.range,
        }
    }
}

/// What `shaders/text.slang` takes for each glyph, an instance of its draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    color: [f32; 4],
    rect: [Vec2; 2],
    field: Texture,
    range: f32,
}
