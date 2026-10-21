//! Sprites: rectangles of textures, tinted, which the engine draws on a quad of its own with a
//! shader of its own.

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3, Vec4};

use crate::{Mesh, Sampler, Shader, Texture};

/// The sprite shader, which the build compiled from `shaders/sprite.slang`.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sprite.spv"));

/// What a sprite shows: a rectangle of a texture, tinted. A sprite is a square a unit across,
/// centered on its origin in its XY plane and facing +Z, with `rect`'s top left at its top left,
/// +Y being up; the draw's transform places, sizes and turns it in the world, and it's drawn by
/// straight alpha over what's drawn before it. `Sprite::default()` shows the whole of the engine's
/// white texture, untinted, so that a sprite of only a color is `Sprite { color,
/// ..Sprite::default() }`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    /// In linear RGBA, alpha not multiplied in, which multiplies the texture's colors and alpha:
    /// over the white texture, the sprite's own color. White by default.
    pub color: Vec4,
    /// The engine's white texture by default.
    pub texture: Texture,
    /// The part of `texture` to show, its top left then its bottom right, in texture coordinates,
    /// from (0, 0) at the texture's top left to (1, 1) at its bottom right: the whole texture by
    /// default. Past 0 or 1, a repeating sampler tiles the texture; a left past its right, or a top
    /// below its bottom, flips it. Frames cut from a sprite sheet need a texel of space between
    /// them: a linear sampler mixes in the texels just outside the rectangle at its edges, and a
    /// nearest one can take one where a sprite's edge lies on a pixel's center.
    pub rect: [Vec2; 2],
    /// How `texture` is sampled: [`Sampler::LinearClamp`] by default.
    pub sampler: Sampler,
}

impl Default for Sprite {
    fn default() -> Self {
        Self {
            color: Vec4::ONE,
            texture: Texture::WHITE,
            rect: [Vec2::ZERO, Vec2::ONE],
            sampler: Sampler::LinearClamp,
        }
    }
}

impl Sprite {
    /// What the sprite shader takes with each draw.
    pub(crate) fn params(&self) -> Params {
        Params {
            color: self.color.to_array(),
            rect: self.rect,
            texture: self.texture,
            sampler: self.sampler.index(),
        }
    }
}

/// A corner of the quad, which `shaders/sprite.slang` reads: its position in the quad's own space,
/// and where it is in the sprite's rectangle, from (0, 0) at its top left to (1, 1) at its bottom
/// right.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Corner {
    position: Vec3,
    uv: Vec2,
}

/// The quad: a unit square centered on the origin, facing +Z, its rectangle upright on it.
pub(crate) const CORNERS: [Corner; 4] = [
    Corner {
        position: Vec3::new(-0.5, -0.5, 0.0),
        uv: Vec2::new(0.0, 1.0),
    },
    Corner {
        position: Vec3::new(0.5, -0.5, 0.0),
        uv: Vec2::new(1.0, 1.0),
    },
    Corner {
        position: Vec3::new(0.5, 0.5, 0.0),
        uv: Vec2::new(1.0, 0.0),
    },
    Corner {
        position: Vec3::new(-0.5, 0.5, 0.0),
        uv: Vec2::new(0.0, 0.0),
    },
];
pub(crate) const INDICES: [u32; 6] = [0, 1, 2, 2, 3, 0];

/// What `shaders/sprite.slang` takes with each draw. The color is an array, since a `Vec4` is
/// 16-byte aligned, which would pad the struct from 40 bytes to 48.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    color: [f32; 4],
    rect: [Vec2; 2],
    texture: Texture,
    sampler: u32,
}

/// The quad, the first mesh of every `Meshes`.
pub(crate) const QUAD: Mesh<Corner> = Mesh::new(0);

/// The sprite shader, the first shader of every `Shaders`.
pub(crate) const SHADER: Shader<Corner, Params> = Shader::new(0);
