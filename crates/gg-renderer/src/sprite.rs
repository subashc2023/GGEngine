//! Sprites: tinted rectangles of textures, drawn on the engine's quad by their own shader; and the
//! sprite sheets whose cells they show.

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3, Vec4};

use crate::{Mesh, Sampler, Shader, Texture};

/// `shaders/sprite.slang`, compiled by the build.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sprite.spv"));

/// A tinted rectangle of a texture, on a unit square centered on its origin in its XY plane, facing
/// +Z.
///
/// `rect`'s top left is at its top left (+Y up). The draw's transform places, sizes and turns it.
/// It blends by straight alpha over what's drawn before it, testing depth without writing it.
/// `Sprite::default()` shows the engine's white texture untinted, so a plain color is `Sprite {
/// color, ..Sprite::default() }`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    /// Linear RGBA, alpha not premultiplied, multiplying the texture's colors and alpha. White by
    /// default.
    pub color: Vec4,
    /// `None` (the default) is the engine's white texture, a pixel across.
    pub texture: Option<Texture>,
    /// The part of `texture` shown, top left then bottom right, in texture coordinates: (0, 0) at
    /// its top left, (1, 1) at its bottom right. The whole texture by default; a sheet's cells from
    /// [`SpriteSheet::sprite`]. Past 0 or 1, a repeating sampler tiles it; a left past its right,
    /// or a top below its bottom, flips it.
    ///
    /// The sprite samples only texels the rectangle covers, never nearer its edges than their
    /// centers. So under either sampler, cells packed edge to edge show nothing of each other, a
    /// tiled sprite's edges don't blend with what they'd wrap to, and linear sprites side by side
    /// don't blend.
    pub rect: [Vec2; 2],
    /// [`Sampler::LinearClamp`] by default.
    pub sampler: Sampler,
}

impl Default for Sprite {
    fn default() -> Self {
        Self {
            color: Vec4::ONE,
            texture: None,
            rect: [Vec2::ZERO, Vec2::ONE],
            sampler: Sampler::LinearClamp,
        }
    }
}

/// A texture cut into `cell`-pixel cells packed edge to edge from its top left.
///
/// As tile sets and animation frames come. Cells a pixel across allow any rectangle of pixels, as
/// an atlas of images of different sizes. `size` is the texture's, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpriteSheet {
    pub texture: Texture,
    /// Across, then down.
    pub size: [u32; 2],
    /// Across, then down.
    pub cell: [u32; 2],
}

impl SpriteSheet {
    /// A sprite of `span` cells across and down from `at` (column, row from the top left): `[1, 1]`
    /// for one cell. The rest is `Sprite::default()`'s: untinted and filtered linearly. Pixel art
    /// wants [`Sampler::NearestClamp`].
    ///
    /// # Panics
    ///
    /// - The cells reach past the sheet's edges.
    /// - `span` or `cell` has a side of 0.
    pub fn sprite(&self, at: [u32; 2], span: [u32; 2]) -> Sprite {
        // The cells' start and end on an axis, as fractions of the sheet. Pixels count in u64,
        // where only the end can overflow, and an overflow fails the check.
        let fractions = |axis: usize| {
            let (cell, size) = (u64::from(self.cell[axis]), u64::from(self.size[axis]));
            let start = u64::from(at[axis]) * cell;
            let end = (u64::from(at[axis]) + u64::from(span[axis])).checked_mul(cell);
            let Some(end) = end.filter(|&end| start < end && end <= size) else {
                panic!(
                    "cells from {at:?}, {span:?} across and down, of {:?} pixels, don't fit a \
                     sprite sheet of {:?} pixels",
                    self.cell, self.size
                );
            };
            [start as f32 / size as f32, end as f32 / size as f32]
        };
        let [left, right] = fractions(0);
        let [top, bottom] = fractions(1);
        Sprite {
            texture: Some(self.texture),
            rect: [Vec2::new(left, top), Vec2::new(right, bottom)],
            ..Sprite::default()
        }
    }
}

impl Sprite {
    pub(crate) fn params(&self) -> Params {
        Params {
            color: self.color.to_array(),
            rect: self.rect,
            texture: self.texture.unwrap_or(Texture::WHITE),
            sampler: self.sampler.index(),
        }
    }
}

/// A corner of the quad: `Corner` in `shaders/mesh.slang`, read by the sprite, circle and line
/// shaders. Its position in the quad's space, and its place in the sprite's rectangle, (0, 0) top
/// left to (1, 1) bottom right.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Corner {
    pub(crate) position: Vec3,
    uv: Vec2,
}

/// A unit square centered on the origin, facing +Z, its rectangle upright.
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

/// What `shaders/sprite.slang` takes per sprite, an instance of its draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    color: [f32; 4],
    rect: [Vec2; 2],
    texture: Texture,
    sampler: u32,
}

/// The quad, every `Meshes`' first mesh.
pub(crate) const QUAD: Mesh<Corner> = Mesh::new(0);

/// The sprite shader, every `Shaders`' first shader.
pub(crate) const SHADER: Shader<Corner, Params> = Shader::new(0, false);

#[cfg(test)]
mod tests {
    use super::*;

    /// The Cherno's example: 128-pixel cells on a sheet 2,560 pixels across.
    const SHEET: SpriteSheet = SpriteSheet {
        texture: Texture::WHITE,
        size: [2560, 1664],
        cell: [128, 128],
    };

    #[test]
    fn a_sheets_cells_are_its_pixels_over_its_size_from_its_top_left() {
        // The third column and the fourth row: from 256 pixels in and 384 down.
        let cell = SHEET.sprite([2, 3], [1, 1]);
        let top_left = Vec2::new(0.1, 384.0 / 1664.0);
        assert_eq!(cell.rect, [top_left, Vec2::new(0.15, 512.0 / 1664.0)]);
        // A tree two cells tall.
        let tree = SHEET.sprite([2, 3], [1, 2]);
        assert_eq!(tree.rect, [top_left, Vec2::new(0.15, 640.0 / 1664.0)]);
        assert_eq!(
            tree,
            Sprite {
                texture: Some(SHEET.texture),
                rect: tree.rect,
                ..Sprite::default()
            }
        );
        // With cells a pixel across, any rectangle of pixels.
        let pixels = SpriteSheet {
            cell: [1, 1],
            ..SHEET
        };
        assert_eq!(pixels.sprite([256, 384], [128, 256]).rect, tree.rect);
    }

    #[test]
    #[should_panic(expected = "don't fit a sprite sheet")]
    fn cells_past_the_sheet_are_a_bug() {
        // The sheet is 20 cells across.
        SHEET.sprite([19, 0], [2, 1]);
    }

    #[test]
    #[should_panic(expected = "don't fit a sprite sheet")]
    fn a_span_of_no_cells_is_a_bug() {
        SHEET.sprite([2, 3], [1, 0]);
    }

    #[test]
    #[should_panic(expected = "don't fit a sprite sheet")]
    fn cells_whose_pixels_overflow_are_a_bug() {
        let huge = SpriteSheet {
            size: [u32::MAX, 1664],
            cell: [u32::MAX, 128],
            ..SHEET
        };
        // Their right edge is past 2 to the 64th pixels.
        huge.sprite([u32::MAX, 0], [3, 1]);
    }
}
