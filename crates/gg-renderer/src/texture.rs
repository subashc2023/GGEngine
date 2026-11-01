//! Textures: images that shaders sample, games' and the engine's white one, whose handles are their
//! places in the bindless set, so that a shader's parameters carry them as they are.

use std::array;
use std::borrow::Cow;

use bytemuck::NoUninit;
use gg_core::math::libm;
use gg_core::{Error, Result};
use gg_gpu::{MAX_TEXTURE_SIZE, PLACED_TEXTURES, mip_sizes};

/// A texture that [`Textures::add`] handed out, or the engine's white one. It's a field of a
/// shader's parameters as it is: a `uint` in the shader, the texture's index in the bindless set,
/// which the shader samples as `textures[index]`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, NoUninit)]
pub struct Texture {
    pub(crate) index: u32,
}

impl Texture {
    /// The engine's white texture, a pixel across, which a sprite of only a color shows.
    pub const WHITE: Self = Self { index: 0 };

    /// Its index, the order [`Textures::add`] handed it out in, from 0, the engine's white
    /// texture's: what finds its name, where the handle is the renderer's own.
    pub fn index(self) -> u32 {
        self.index
    }
}

/// Whether a texture has smaller mip levels, which shaders sample where it's drawn smaller than it
/// is rather than shimmering, and how they're made from it, as a frame copies it to the GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mipmaps {
    /// It has one level, as sprites' textures do: a sheet's cells would mix in smaller ones.
    None,
    /// Each level averages the one before's colors in linear space, and its alpha: for a texture
    /// whose alpha nothing reads, such as an opaque material's, whose transparent pixels' colors
    /// still show.
    Averaged,
    /// Each level averages the one before's colors weighted by their alpha, so that a transparent
    /// pixel's color, which shows nothing, doesn't bleed into those beside it, and its alpha: for a
    /// texture whose alpha is coverage, as a masked or blended material's is.
    AlphaWeighted,
}

/// The textures a renderer's shaders sample, the first of them the engine's white texture. It
/// hands out each one's handle at once, and keeps its pixels until
/// [`Uploads::take`](crate::Uploads::take) takes them, for a frame to copy to the GPU. A renderer
/// samples the textures of one `Textures` only.
pub struct Textures {
    /// Those added since they were last taken, in order.
    pub(crate) added: Vec<Added>,
    /// How many have been added.
    count: u32,
}

pub(crate) struct Added {
    pub(crate) index: u32,
    pub(crate) name: String,
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Vec<u8>,
    pub(crate) mipmaps: Mipmaps,
}

impl Textures {
    /// Adds a texture `size` pixels across and down. `pixels` holds its rows of RGBA8, tightly
    /// packed, top row first, as images store them: colors sRGB-encoded, and alpha not multiplied
    /// into them. It keeps a `Vec` of pixels as it is, and copies borrowed ones. Shaders sample its
    /// colors decoded to linear, with texture coordinates from (0, 0) at its top left to (1, 1) at
    /// its bottom right. `mipmaps` says whether it has smaller levels too, made from it as a frame
    /// copies it to the GPU, and how. `name` names it in errors and GPU captures.
    ///
    /// Fails with `renderer.bad_texture` if a side is 0 or more than 8,192 pixels, the most every
    /// Vulkan 1.4 device takes, or `pixels` doesn't fill it exactly; and with
    /// `renderer.too_many_textures` past the 61,440 textures a renderer holds, the engine's white
    /// one among them, which [`room`](Self::room) counts down.
    pub fn add<'a>(
        &mut self,
        name: &str,
        size: [u32; 2],
        pixels: impl Into<Cow<'a, [u8]>>,
        mipmaps: Mipmaps,
    ) -> Result<Texture> {
        let pixels = pixels.into();
        let bad = |problem: String| {
            Error::new(
                "renderer.bad_texture",
                format!("texture {name:?} {problem}"),
            )
        };
        let [width, height] = size;
        let sides = 1..=MAX_TEXTURE_SIZE;
        if !sides.contains(&width) || !sides.contains(&height) {
            return Err(bad(format!(
                "is {width}x{height} pixels, but textures are 1 to {MAX_TEXTURE_SIZE} pixels a side"
            )));
        }
        let bytes = u64::from(width) * u64::from(height) * 4;
        if pixels.len() as u64 != bytes {
            return Err(bad(format!(
                "is {width}x{height} pixels, {bytes} bytes of RGBA8, but has {} bytes",
                pixels.len()
            )));
        }
        // Its handle is its slot, so a renderer holds as many as the bindless set has placed slots.
        if self.count == PLACED_TEXTURES {
            return Err(Error::new(
                "renderer.too_many_textures",
                format!(
                    "texture {name:?} is one more than the {PLACED_TEXTURES} a renderer holds, \
                     the engine's white one among them"
                ),
            ));
        }
        let index = self.count;
        self.count += 1;
        self.added.push(Added {
            index,
            name: name.to_owned(),
            size,
            pixels: pixels.into_owned(),
            mipmaps,
        });
        Ok(Texture { index })
    }

    /// How many more textures it holds, so that what adds several can check they fit before it
    /// adds any.
    pub fn room(&self) -> u32 {
        PLACED_TEXTURES - self.count
    }

    /// How many it has handed out, the engine's white texture among them: the index of the next.
    pub fn count(&self) -> u32 {
        self.count
    }
}

/// Appends to `pixels`, the RGBA8 of a texture `size` pixels across and down, colors sRGB-encoded
/// and alpha not multiplied into them, the rest of its full chain of mip levels, as `mipmaps`
/// says, smallest last: each level averages the one before by area, each of its texels the texels
/// it covers, wholly or in part, in linear space, as shaders filter them. The levels are kept as
/// floats until each is encoded, so that each comes from the first's exact colors rather than from
/// the level before's rounded bytes. The first stays bytes, as it's the largest: a level of floats
/// takes four times the memory, 16 MiB for a 2,048 by 2,048 texture's second.
pub(crate) fn add_mipmaps(pixels: &mut Vec<u8>, size: [u32; 2], mipmaps: Mipmaps) {
    let weighted = match mipmaps {
        Mipmaps::None => return,
        Mipmaps::Averaged => false,
        Mipmaps::AlphaWeighted => true,
    };
    let _zone = gg_core::zone!();
    let decode: [f32; 256] = array::from_fn(|byte| srgb_to_linear(byte as f32 / 255.0));
    // The linear values at which encoding rounds up to each byte from the one below, so that a
    // search encodes, rounding as GPUs do, rather than a power for each channel. The search is
    // written out, since `partition_point`'s calls take longer than it in debug builds.
    let rounding: [f32; 255] = array::from_fn(|byte| srgb_to_linear((byte as f32 + 0.5) / 255.0));
    let encode = |linear: f32| {
        let (mut low, mut high) = (0, rounding.len());
        while low < high {
            let middle = (low + high) / 2;
            if rounding[middle] <= linear {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low as u8
    };
    let append = |pixels: &mut Vec<u8>, level: &[[f32; 4]]| {
        pixels.reserve(level.len() * 4);
        for &[r, g, b, a] in level {
            let alpha = (a * 255.0).round() as u8;
            pixels.extend_from_slice(&[encode(r), encode(g), encode(b), alpha]);
        }
    };
    let mut sizes = mip_sizes(size[0], size[1]);
    let first_size = sizes.next().expect("a chain has its first level");
    let Some(mut from) = sizes.next() else {
        // A pixel across and down: the whole chain.
        return;
    };
    let first = pixels.as_chunks::<4>().0;
    let texel = |i| {
        let [r, g, b, a]: [u8; 4] = first[i];
        let alpha = f32::from(a) / 255.0;
        [
            decode[usize::from(r)],
            decode[usize::from(g)],
            decode[usize::from(b)],
            alpha,
        ]
    };
    let mut level = shrink(texel, first_size, from, weighted);
    append(pixels, &level);
    for to in sizes {
        level = shrink(|i| level[i], from, to, weighted);
        append(pixels, &level);
        from = to;
    }
}

/// The next mip level, `to` texels across and down, of a level `from` texels across and down whose
/// `i`th texel, row after row, is `texel(i)`: linear RGB, alpha not multiplied in, then alpha.
/// Each texel averages the ones it covers by area, their colors weighted by their alpha if
/// `weighted`, or plainly where they're all transparent. Its loops are written out for debug
/// builds, where a 2,048 by 2,048 texture's levels took seconds through iterators.
fn shrink(
    texel: impl Fn(usize) -> [f32; 4],
    [width, height]: [u32; 2],
    [to_width, to_height]: [u32; 2],
    weighted: bool,
) -> Vec<[f32; 4]> {
    let columns: Vec<_> = (0..to_width).map(|x| taps(width, x)).collect();
    let mut level = Vec::with_capacity(to_width as usize * to_height as usize);
    for y in 0..to_height {
        let (rows, row_count) = taps(height, y);
        for &(columns, column_count) in &columns {
            let (mut color, mut plain, mut weights, mut alpha) = ([0.0; 3], [0.0; 3], 0.0, 0.0);
            for &(row, below) in &rows[..row_count] {
                let row = row * width as usize;
                for &(column, across) in &columns[..column_count] {
                    let area = below * across;
                    let [r, g, b, a] = texel(row + column);
                    let weight = if weighted { area * a } else { area };
                    color[0] += weight * r;
                    color[1] += weight * g;
                    color[2] += weight * b;
                    plain[0] += area * r;
                    plain[1] += area * g;
                    plain[2] += area * b;
                    weights += weight;
                    alpha += area * a;
                }
            }
            let [r, g, b] = if weights > 0.0 {
                color.map(|sum| sum / weights)
            } else {
                plain
            };
            level.push([r, g, b, alpha]);
        }
    }
    level
}

/// The texels along a side `from` texels long that texel `i` along the next level's covers, and
/// how much of it each covers, which add up to 1, then how many of the three there are: two halves
/// where `from` is even; where it's odd, `2m + 1`, the next level has `m`, each covering `from / m`
/// texels, so three, the outer two in part; and the one where it's 1, as the next level's is too.
fn taps(from: u32, i: u32) -> ([(usize, f32); 3], usize) {
    let at = 2 * i as usize;
    if from == 1 {
        ([(0, 1.0); 3], 1)
    } else if from.is_multiple_of(2) {
        ([(at, 0.5), (at + 1, 0.5), (at, 0.5)], 2)
    } else {
        let (m, n, i) = ((from / 2) as f32, from as f32, i as f32);
        (
            [(at, (m - i) / n), (at + 1, m / n), (at + 2, (i + 1.0) / n)],
            3,
        )
    }
}

/// An sRGB-encoded channel, from 0 to 1, decoded to linear, as Vulkan's sRGB formats decode it.
fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        libm::powf((encoded + 0.055) / 1.055, 2.4)
    }
}

impl Default for Textures {
    fn default() -> Self {
        let mut textures = Self {
            added: Vec::new(),
            count: 0,
        };
        let white = textures
            .add("white", [1, 1], &[255; 4], Mipmaps::None)
            .expect("a texture a pixel across");
        assert_eq!(white, Texture::WHITE, "the white texture comes first");
        textures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_texture_is_filled_by_its_pixels_and_fits_every_gpu() {
        let mut textures = Textures::default();
        let past = MAX_TEXTURE_SIZE + 1;
        let bad = [
            ([0, 1], 0),
            ([past, 1], past as usize * 4),
            ([2, 2], 15),
            ([2, 2], 17),
        ];
        for (size, bytes) in bad {
            let error = textures
                .add("broken", size, vec![0; bytes], Mipmaps::None)
                .expect_err("a broken texture");
            assert_eq!(
                error.code(),
                "renderer.bad_texture",
                "{size:?}, {bytes} bytes"
            );
            assert!(error.to_string().contains("\"broken\""), "{error}");
        }
        let mut add = || {
            textures
                .add("a", [2, 1], &[0; 8], Mipmaps::Averaged)
                .map(|texture| texture.index)
        };
        // After the engine's white texture.
        assert_eq!(add().ok(), Some(1));
        assert_eq!(add().ok(), Some(2));
    }

    #[test]
    fn a_renderer_holds_as_many_textures_as_the_placed_slots() {
        let mut textures = Textures::default();
        // The engine's white texture takes the first.
        assert_eq!(textures.room(), PLACED_TEXTURES - 1);
        for _ in 1..PLACED_TEXTURES {
            let added = textures.add("a", [1, 1], &[0; 4], Mipmaps::None);
            added.expect("a slot for it");
        }
        assert_eq!(textures.room(), 0);
        let error = textures
            .add("one more", [1, 1], &[0; 4], Mipmaps::None)
            .expect_err("no slot left");
        assert_eq!(error.code(), "renderer.too_many_textures");
    }

    /// The levels after the first that `pixels`, `size` pixels across and down, gets.
    fn levels(pixels: &[u8], size: [u32; 2], mipmaps: Mipmaps) -> Vec<u8> {
        let mut levels = pixels.to_vec();
        add_mipmaps(&mut levels, size, mipmaps);
        levels.split_off(pixels.len())
    }

    #[test]
    fn levels_average_in_linear_space_by_area() {
        use Mipmaps::{AlphaWeighted, Averaged};
        let (white, black) = ([255; 4], [0, 0, 0, 255]);
        // Half white light is 188 encoded, where averaging the encoded bytes would give 128.
        let checker = [white, black, black, white].concat();
        assert_eq!(levels(&checker, [2, 2], Averaged), [188, 188, 188, 255]);
        // Three texels into one: each a third, none skipped.
        let row = [white, black, black].concat();
        assert_eq!(levels(&row, [3, 1], Averaged), [156, 156, 156, 255]);
        // Five into two, the middle texel half in each: 0.8 of white light, then none, then 0.4.
        let row = [white, white, black, black, black].concat();
        let expected = [[231, 231, 231, 255], black, [170, 170, 170, 255]].concat();
        assert_eq!(levels(&row, [5, 1], Averaged), expected);
        assert_eq!(levels(&row, [5, 1], AlphaWeighted), expected);
        // The last level from the first's colors: averaging the second's bytes would give 137.
        let gray = |value| [value, value, value, 255];
        let row = [gray(0), gray(0), gray(26), gray(255)].concat();
        let expected = [gray(0), gray(188), gray(138)].concat();
        assert_eq!(levels(&row, [4, 1], Averaged), expected);
    }

    #[test]
    fn alpha_weighted_levels_keep_transparent_colors_out() {
        use Mipmaps::{AlphaWeighted, Averaged};
        let (red, clear_green) = ([255, 0, 0, 255], [0, 255, 0, 0]);
        let pair = [red, clear_green].concat();
        assert_eq!(levels(&pair, [2, 1], AlphaWeighted), [255, 0, 0, 128]);
        assert_eq!(levels(&pair, [2, 1], Averaged), [188, 188, 0, 128]);
        // Where everything is transparent, the colors average plainly, rather than dividing by 0.
        let clear = [[255, 0, 0, 0], [0, 0, 255, 0]].concat();
        assert_eq!(levels(&clear, [2, 1], AlphaWeighted), [188, 0, 188, 0]);
    }

    #[test]
    fn every_level_of_the_chain_follows() {
        let pixels = vec![200; 211 * 5 * 4];
        let all: usize = mip_sizes(211, 5).map(|[w, h]| (w * h * 4) as usize).sum();
        let levels = levels(&pixels, [211, 5], Mipmaps::AlphaWeighted);
        assert_eq!(levels.len(), all - pixels.len());
        // A flat color stays itself, level after level.
        assert!(levels.iter().all(|&byte| byte == 200), "{levels:?}");
        assert!(self::levels(&pixels, [211, 5], Mipmaps::None).is_empty());
    }
}
