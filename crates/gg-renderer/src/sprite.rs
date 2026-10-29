//! Sprites: rectangles of textures, tinted, which the engine draws on a quad of its own with a
//! shader of its own; and the sprite sheets whose cells they show.

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3, Vec4};

use crate::{Mesh, Sampler, Shader, Texture};

/// The sprite shader, which the build compiled from `shaders/sprite.slang`.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sprite.spv"));

/// What a sprite shows: a rectangle of a texture, tinted. A sprite is a square a unit across,
/// centered on its origin in its XY plane and facing +Z, with `rect`'s top left at its top left,
/// +Y being up; the draw's transform places, sizes and turns it in the world, and it's drawn by
/// straight alpha over what's drawn before it, but where an opaque mesh is nearer, testing depth
/// without writing it. `Sprite::default()` shows the whole of the engine's white texture,
/// untinted, so that a sprite of only a color is `Sprite { color, ..Sprite::default() }`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    /// In linear RGBA, alpha not multiplied in, which multiplies the texture's colors and alpha:
    /// over the white texture, the sprite's own color. White by default.
    pub color: Vec4,
    /// The engine's white texture by default.
    pub texture: Texture,
    /// The part of `texture` to show, its top left then its bottom right, in texture coordinates,
    /// from (0, 0) at the texture's top left to (1, 1) at its bottom right: the whole texture by
    /// default, and a sheet's cells from [`SpriteSheet::sprite`]. Past 0 or 1, a repeating sampler
    /// tiles the texture; a left past its right, or a top below its bottom, flips it. The sprite
    /// samples only the texels the rectangle covers, wholly or in part, never nearer its edges
    /// than their centers, so that under a linear or a nearest sampler the cells of a sheet packed
    /// edge to edge show nothing of one another. So a tiled sprite's outermost texels don't blend
    /// with those they'd wrap to either, nor, under a linear sampler, sprites side by side with
    /// each other.
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

/// A texture cut into cells `cell` pixels across and down, packed edge to edge from its top left,
/// as tile sets and the frames of animations come; with cells a pixel across, into any rectangle
/// of its pixels, as an atlas holds images of different sizes. `size` is the texture's, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpriteSheet {
    pub texture: Texture,
    /// Across, then down.
    pub size: [u32; 2],
    /// Across, then down.
    pub cell: [u32; 2],
}

impl SpriteSheet {
    /// A sprite of the cells from `at`, its column then its row counted from the sheet's top left,
    /// `span` cells across and down: `[1, 1]` for one cell. Its other fields are
    /// `Sprite::default()`'s, so it's untinted and filtered linearly; pixel art wants
    /// [`Sampler::NearestClamp`].
    ///
    /// # Panics
    ///
    /// If the cells reach past the sheet's edges, or `span` or `cell` has a side of 0.
    pub fn sprite(&self, at: [u32; 2], span: [u32; 2]) -> Sprite {
        // Where the cells start and end on an axis, as a fraction of the sheet; pixels are counted
        // in u64, where only the end can overflow, and a product that does fails the check.
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
            texture: self.texture,
            rect: [Vec2::new(left, top), Vec2::new(right, bottom)],
            ..Sprite::default()
        }
    }
}

impl Sprite {
    /// What the sprite shader takes for the sprite, an instance of its draw.
    pub(crate) fn params(&self) -> Params {
        Params {
            color: self.color.to_array(),
            rect: self.rect,
            texture: self.texture,
            sampler: self.sampler.index(),
        }
    }
}

/// A corner of the quad, which the sprite, circle and line shaders read as `Corner` in
/// `shaders/mesh.slang`: its position in the quad's own space, and where it is in the sprite's
/// rectangle, from (0, 0) at its top left to (1, 1) at its bottom right.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Corner {
    pub(crate) position: Vec3,
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

/// What `shaders/sprite.slang` takes for each sprite, an instance of its draw.
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
pub(crate) const SHADER: Shader<Corner, Params> = Shader::new(0, false);

#[cfg(test)]
mod tests {
    use super::*;

    /// The Cherno's example: cells of 128 pixels, on a sheet 2,560 pixels across.
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
                texture: SHEET.texture,
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
