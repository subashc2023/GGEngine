//! Textures: images that shaders sample, games' and the engine's white one, whose handles are their
//! places in the bindless set, so that a shader's parameters carry them as they are.

use std::array;
use std::borrow::Cow;

use bytemuck::NoUninit;
use gg_core::math::libm;
use gg_core::{Error, Result};
use gg_gpu::{MAX_TEXTURE_SIZE, PLACED_TEXTURES, TextureFormat, mip_sizes};

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

/// What a texture's texels are: colors, sRGB-encoded as images store them, which shaders sample
/// decoded to linear, with smaller mip levels as [`Mipmaps`] says, which a `Mipmaps` stands for;
/// data, such as a font's distance field, which shaders read as stored, each channel a byte from 0
/// to 1, with one level; or colors compressed as BC7, as a game's package holds its textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Texels {
    Color(Mipmaps),
    Data,
    /// Colors as [`Color`](Self::Color)'s are, sRGB-encoded and alpha not multiplied into them,
    /// compressed as BC7: blocks of 4 by 4 texels, 16 bytes each, row after row, top row first,
    /// of each of its levels, largest first, a block covering a level's edge where its side isn't
    /// a multiple of 4. Its first level alone, or, if `mipmapped`, the full chain [`mip_sizes`]
    /// gives, which the cooker made: a frame makes none.
    Bc7 {
        mipmapped: bool,
    },
}

impl From<Mipmaps> for Texels {
    fn from(mipmaps: Mipmaps) -> Self {
        Self::Color(mipmaps)
    }
}

impl Texels {
    /// The levels a frame makes from the first: none for data, nor for BC7, which comes with them.
    pub(crate) fn mipmaps(self) -> Mipmaps {
        match self {
            Self::Color(mipmaps) => mipmaps,
            Self::Data | Self::Bc7 { .. } => Mipmaps::None,
        }
    }

    /// Whether the texture has the full chain of mip levels, which a frame makes or BC7 comes with.
    pub(crate) fn mipmapped(self) -> bool {
        match self {
            Self::Color(mipmaps) => mipmaps != Mipmaps::None,
            Self::Data => false,
            Self::Bc7 { mipmapped } => mipmapped,
        }
    }

    /// The format of the texture's image on the GPU.
    pub(crate) fn format(self) -> TextureFormat {
        match self {
            Self::Color(_) => TextureFormat::Rgba8Srgb,
            Self::Data => TextureFormat::Rgba8Unorm,
            Self::Bc7 { .. } => TextureFormat::Bc7Srgb,
        }
    }

    /// How many bytes a texture `size` texels across and down takes of these: its first level's
    /// RGBA8 for colors and data, from which a frame makes the rest, and every level's blocks for
    /// BC7.
    fn bytes(self, size: [u32; 2]) -> u64 {
        match self {
            Self::Color(_) | Self::Data => TextureFormat::Rgba8Srgb.bytes(size, false),
            Self::Bc7 { mipmapped } => TextureFormat::Bc7Srgb.bytes(size, mipmapped),
        }
    }
}

/// The textures a renderer's shaders sample, the first of them the engine's white texture. It
/// hands out each one's handle at once, and keeps its pixels until
/// [`Uploads::take`](crate::Uploads::take) takes them, for a frame to copy to the GPU; a texture
/// replaced by another image the same way. A renderer samples the textures of one `Textures` only.
pub struct Textures {
    /// Those added or replaced since they were last taken, in order.
    pub(crate) added: Vec<Added>,
    /// Whether each is data, by its index, which its replacements keep to.
    data: Vec<bool>,
}

pub(crate) struct Added {
    pub(crate) index: u32,
    /// Whether it's another image in place of one a renderer has, rather than one added, which
    /// comes after those a renderer has.
    pub(crate) replacement: bool,
    pub(crate) name: String,
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Vec<u8>,
    pub(crate) texels: Texels,
}

impl Textures {
    /// Adds a texture `size` pixels across and down. `pixels` holds its rows of RGBA8, tightly
    /// packed, top row first, as images store them, which are colors or data as `texels` says: for
    /// colors, sRGB-encoded, and alpha not multiplied into them, which shaders sample decoded to
    /// linear, with smaller levels too if the [`Mipmaps`] it gives say so, made from it as a frame
    /// copies it to the GPU, and how; for data, read as stored; and for BC7, its levels' blocks
    /// instead, as [`Texels::Bc7`] says. It keeps a `Vec` of pixels as it is, and copies borrowed
    /// ones. Shaders sample it with texture coordinates from (0, 0) at its top left to (1, 1) at
    /// its bottom right. `name` names it in errors and GPU captures.
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
        texels: impl Into<Texels>,
    ) -> Result<Texture> {
        let (pixels, texels) = (pixels.into(), texels.into());
        check(name, size, &pixels, texels)?;
        // Its handle is its slot, so a renderer holds as many as the bindless set has placed slots.
        let index = self.count();
        if index == PLACED_TEXTURES {
            return Err(Error::new(
                "renderer.too_many_textures",
                format!(
                    "texture {name:?} is one more than the {PLACED_TEXTURES} a renderer holds, \
                     the engine's white one among them"
                ),
            ));
        }
        self.data.push(texels == Texels::Data);
        self.added.push(Added {
            index,
            replacement: false,
            name: name.to_owned(),
            size,
            pixels: pixels.into_owned(),
            texels,
        });
        Ok(Texture { index })
    }

    /// Puts another image in place of `texture`'s, which keeps its handle: `size` pixels across and
    /// down, whatever size it had, its `pixels` and `texels` as [`add`](Self::add) takes them, so
    /// that what samples it shows them from the frame that copies them to the GPU on, which waits
    /// for the frames before it, as they may sample the old. It takes the place of any of that
    /// texture's pixels still waiting for a frame, as it was added or replaced. `name` names it in
    /// errors.
    ///
    /// Fails with `renderer.bad_texture` as `add` does, leaving the texture as it was.
    ///
    /// # Panics
    ///
    /// If `texture` isn't one of these textures, as another `Textures`' handles can be past them,
    /// or `texels` are data where its texels are colors, or colors, compressed or not, where
    /// they're data: a texture keeps the kind it was added as.
    pub fn replace<'a>(
        &mut self,
        name: &str,
        texture: Texture,
        size: [u32; 2],
        pixels: impl Into<Cow<'a, [u8]>>,
        texels: impl Into<Texels>,
    ) -> Result<()> {
        let Some(&data) = self.data.get(texture.index as usize) else {
            panic!("texture {name:?} replaces one of another `Textures`");
        };
        let texels = texels.into();
        assert_eq!(
            texels == Texels::Data,
            data,
            "texture {name:?} is replaced by texels of the other kind"
        );
        let pixels = pixels.into();
        check(name, size, &pixels, texels)?;
        let pixels = pixels.into_owned();
        match (self.added.iter_mut()).find(|added| added.index == texture.index) {
            Some(waiting) => {
                waiting.size = size;
                waiting.pixels = pixels;
                waiting.texels = texels;
            }
            None => self.added.push(Added {
                index: texture.index,
                replacement: true,
                name: name.to_owned(),
                size,
                pixels,
                texels,
            }),
        }
        Ok(())
    }

    /// How many more textures it holds, so that what adds several can check they fit before it
    /// adds any.
    pub fn room(&self) -> u32 {
        PLACED_TEXTURES - self.count()
    }

    /// How many it has handed out, the engine's white texture among them: the index of the next.
    pub fn count(&self) -> u32 {
        self.data.len() as u32
    }
}

/// Checks that `pixels` fill a texture of `texels` named `name`, `size` pixels across and down,
/// that every Vulkan 1.4 device takes, as [`Textures::add`] says.
fn check(name: &str, size: [u32; 2], pixels: &[u8], texels: Texels) -> Result<()> {
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
    let bytes = texels.bytes(size);
    if pixels.len() as u64 != bytes {
        let of = match texels {
            Texels::Color(_) | Texels::Data => "RGBA8",
            Texels::Bc7 { mipmapped: false } => "BC7 blocks",
            Texels::Bc7 { mipmapped: true } => "BC7 blocks in its levels",
        };
        return Err(bad(format!(
            "is {width}x{height} pixels, {bytes} bytes of {of}, but has {} bytes",
            pixels.len()
        )));
    }
    Ok(())
}

/// Appends to `pixels`, the RGBA8 of a texture `size` pixels across and down, colors sRGB-encoded
/// and alpha not multiplied into them, the rest of its full chain of mip levels, as `mipmaps`
/// says, smallest last: each level averages the one before by area, each of its texels the texels
/// it covers, wholly or in part, in linear space, as shaders filter them. The levels are kept as
/// floats until each is encoded, so that each comes from the first's exact colors rather than from
/// the level before's rounded bytes. The first stays bytes, as it's the largest: a level of floats
/// takes four times the memory, 16 MiB for a 2,048 by 2,048 texture's second. The cooker makes a
/// texture's levels with it before it compresses them, as a frame makes those of one it doesn't.
#[doc(hidden)]
pub fn add_mipmaps(pixels: &mut Vec<u8>, size: [u32; 2], mipmaps: Mipmaps) {
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
            data: Vec::new(),
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

    #[test]
    fn a_texture_replaced_keeps_its_handle_and_the_place_of_pixels_waiting() {
        let mut textures = Textures::default();
        let a = textures
            .add("a", [1, 1], &[0; 4], Mipmaps::None)
            .expect("a");
        let waiting = |textures: &Textures| -> Vec<(u32, bool, [u32; 2], Texels)> {
            let added = textures.added.iter();
            added
                .map(|added| (added.index, added.replacement, added.size, added.texels))
                .collect()
        };
        // Still waiting as it was added, which takes the image instead.
        let replaced = textures.replace("a", a, [2, 1], &[9; 8], Mipmaps::Averaged);
        replaced.expect("another image");
        let white = (0, false, [1, 1], Texels::Color(Mipmaps::None));
        let averaged = Texels::Color(Mipmaps::Averaged);
        assert_eq!(waiting(&textures), [white, (1, false, [2, 1], averaged)]);
        // Once a frame has taken it, in place of the one the renderer has.
        textures.added.clear();
        let replaced = textures.replace("a", a, [1, 2], &[7; 8], Mipmaps::None);
        replaced.expect("another image");
        let none = Texels::Color(Mipmaps::None);
        assert_eq!(waiting(&textures), [(1, true, [1, 2], none)]);
        // A broken image leaves it as it was.
        let error = textures
            .replace("a", a, [2, 2], &[0; 4], Mipmaps::None)
            .expect_err("a broken image");
        assert_eq!(error.code(), "renderer.bad_texture");
        assert_eq!(waiting(&textures), [(1, true, [1, 2], none)]);
        assert_eq!(textures.added[0].pixels, [7; 8]);
        // Data, replaced by data.
        let field = textures.add("field", [1, 1], &[0; 4], Texels::Data);
        let field = field.expect("data");
        let replaced = textures.replace("field", field, [2, 1], &[1; 8], Texels::Data);
        replaced.expect("other data");
        let data = (2, false, [2, 1], Texels::Data);
        assert_eq!(waiting(&textures), [(1, true, [1, 2], none), data]);
    }

    #[test]
    fn a_bc7_texture_is_filled_by_its_levels_blocks() {
        let mut textures = Textures::default();
        // 13x5 texels are 4x2 blocks; its chain's 6x2, 3x1 and 1x1 levels a block or two each.
        let (one, chain) = (
            Texels::Bc7 { mipmapped: false },
            Texels::Bc7 { mipmapped: true },
        );
        let blocks = |count: usize| vec![0; count * 16];
        let texture = textures.add("one", [13, 5], blocks(8), one);
        let texture = texture.expect("a level's blocks");
        textures
            .add("chain", [13, 5], blocks(12), chain)
            .expect("every level's blocks");
        for (texels, count) in [(one, 12), (chain, 8), (one, 7)] {
            let error = (textures.add("short", [13, 5], blocks(count), texels))
                .expect_err("blocks that don't fill it");
            assert_eq!(error.code(), "renderer.bad_texture", "{texels:?}, {count}");
        }
        // A texture of colors takes colors compressed in its place, and back.
        let replaced = textures.replace("one", texture, [2, 2], &[0; 16], Mipmaps::None);
        replaced.expect("RGBA8 in place of BC7");
        let replaced = textures.replace("one", texture, [1, 1], blocks(1), chain);
        replaced.expect("BC7 in place of RGBA8");
    }

    #[test]
    #[should_panic(expected = "replaced by texels of the other kind")]
    fn a_texture_keeps_its_kind_of_texels() {
        let mut textures = Textures::default();
        let field = textures.add("field", [1, 1], &[0; 4], Texels::Data);
        let field = field.expect("data");
        let _ = textures.replace("field", field, [1, 1], &[0; 4], Mipmaps::None);
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
