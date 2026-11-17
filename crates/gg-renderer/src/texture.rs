//! Textures shaders sample, games' and the engine's white one. A handle is its place in the
//! bindless set, so shader parameters carry it as is.

use std::array;
use std::borrow::Cow;

use bytemuck::NoUninit;
use gg_core::math::libm;
use gg_core::{Error, Result};
use gg_gpu::{MAX_TEXTURE_SIZE, PLACED_TEXTURES, TextureFormat, mip_sizes};

/// A handle [`Textures::add`] handed out, or the engine's white texture. In shader parameters it's
/// a `uint`, its bindless index, sampled as `textures[index]`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, NoUninit)]
pub struct Texture {
    pub(crate) index: u32,
}

impl Texture {
    /// The engine's white texture, a pixel across: what a color-only sprite shows.
    pub const WHITE: Self = Self { index: 0 };

    /// The order [`Textures::add`] handed it out in, from 0 (the white texture's). It finds the
    /// texture's name where the handle is the renderer's own.
    pub fn index(self) -> u32 {
        self.index
    }
}

/// Whether a texture has smaller mip levels (sampled where it's drawn smaller, rather than
/// shimmering), and how a frame makes them as it copies it to the GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mipmaps {
    /// One level, as sprites' textures have: a sheet's cells would mix in smaller ones.
    None,
    /// Each level averages the last's colors in linear space, and its alpha: for textures whose
    /// alpha nothing reads, as an opaque material's, whose transparent pixels' colors still show.
    Averaged,
    /// Each level averages the last's colors weighted by alpha, so transparent pixels' colors don't
    /// bleed, and its alpha: for textures whose alpha is coverage, as masked or blended materials'.
    AlphaWeighted,
}

/// What a texture's texels are.
///
/// sRGB colors with [`Mipmaps`] (which converts to one) or with levels made; data, as a font's distance field, read as stored bytes from 0 to 1, one level; BC7 colors,
/// as packages hold them; a material's map ([`Map`]) with its full chain, stored or compressed; or
/// HDR colors, as a sky's, one level, stored or compressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Texels {
    Color(Mipmaps),
    /// Colors as [`Color`](Self::Color)'s, with [`mip_sizes`]' full chain made already, each
    /// level's RGBA8 rows after the last's, largest first, as [`add_mipmaps`] makes them (the
    /// engine's loader, off the main thread). A frame makes none.
    ColorLevels,
    Data,
    /// [`Color`](Self::Color)'s colors as BC7: 4 by 4 blocks of 16 bytes, rows top first, per
    /// level, largest first; edge blocks cover a side that isn't a multiple of 4. One level, or if
    /// `mipmapped` the cooker's full [`mip_sizes`] chain. A frame makes none.
    Bc7 {
        mipmapped: bool,
    },
    /// A material's map, read as stored, each channel a byte from 0 to 1: its first level's RGBA8
    /// rows. A frame makes the rest of the chain, each level averaging the last's values by area
    /// ([`add_map_mipmaps`]).
    Map(Map),
    /// A map as [`Map`](Self::Map)'s, its full chain made already by the engine's loader, off the
    /// main thread. A frame makes none.
    MapLevels(Map),
    /// A compressed map, blocks as [`Bc7`](Self::Bc7)'s: BC5 if `bc5` (a [`Map::Normal`]'s X and Y,
    /// as the cooker makes them), else BC7 (the cooker's [`Map::Surface`], or any map in a model's
    /// DDS). One level, or if `mipmapped` the full chain. A frame makes none.
    CompressedMap {
        map: Map,
        bc5: bool,
        mipmapped: bool,
    },
    /// Linear HDR colors, as a `.hdr` image holds them: RGBA rows of 16-bit floats, 8 bytes a
    /// texel, tightly packed, top row first, one level, read as they are.
    Hdr,
    /// HDR colors compressed as unsigned BC6H, as a game's package holds a sky: one level's 4 by 4
    /// blocks of 16 bytes, laid out as [`Bc7`](Self::Bc7)'s, read as [`Hdr`](Self::Hdr)'s with
    /// alpha 1.
    Bc6h,
}

/// What a material's map holds, which picks its stand-in and how the cooker compresses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Map {
    /// A value per channel, as glTF's metallic-roughness map holds roughness in green and metallic
    /// in blue, and its occlusion map occlusion in red. White stands in, leaving a material's
    /// factors as they are. Compressed as BC7.
    Surface,
    /// A normal map: the tangent-space normal's X and Y in red and green, 0 to 1 for -1 to 1;
    /// shaders make Z, ignoring blue. A flat normal stands in. The cooker compresses it as BC5,
    /// which holds the two alone; a model's DDS image may hold it as BC7.
    Normal,
}

/// What a texture's texels stand for, which replacements keep to, and so the stand-in its slot
/// shows until its image arrives: white for colors and surface maps, zeros for data, a flat normal
/// for normal maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Color,
    Data,
    Map(Map),
}

impl From<Mipmaps> for Texels {
    fn from(mipmaps: Mipmaps) -> Self {
        Self::Color(mipmaps)
    }
}

impl Texels {
    pub(crate) fn kind(self) -> Kind {
        match self {
            Self::Color(_) | Self::ColorLevels | Self::Bc7 { .. } | Self::Hdr | Self::Bc6h => {
                Kind::Color
            }
            Self::Data => Kind::Data,
            Self::Map(map) | Self::MapLevels(map) | Self::CompressedMap { map, .. } => {
                Kind::Map(map)
            }
        }
    }

    /// These texels once their levels are made from the first ([`add_levels`]): colors' and maps'
    /// with levels; others as they are.
    pub fn with_levels(self) -> Self {
        match self {
            Self::Color(mipmaps) if mipmaps != Mipmaps::None => Self::ColorLevels,
            Self::Map(map) => Self::MapLevels(map),
            texels => texels,
        }
    }

    /// Whether the texture has the full mip chain, made by a frame or come with the texels.
    pub(crate) fn mipmapped(self) -> bool {
        match self {
            Self::Color(mipmaps) => mipmaps != Mipmaps::None,
            Self::ColorLevels | Self::Map(_) | Self::MapLevels(_) => true,
            Self::Data | Self::Hdr | Self::Bc6h => false,
            Self::Bc7 { mipmapped } | Self::CompressedMap { mipmapped, .. } => mipmapped,
        }
    }

    /// The texture's GPU image format.
    pub(crate) fn format(self) -> TextureFormat {
        match self {
            Self::Color(_) | Self::ColorLevels => TextureFormat::Rgba8Srgb,
            Self::Data | Self::Map(_) | Self::MapLevels(_) => TextureFormat::Rgba8Unorm,
            Self::Bc7 { .. } => TextureFormat::Bc7Srgb,
            Self::CompressedMap { bc5: false, .. } => TextureFormat::Bc7Unorm,
            Self::CompressedMap { bc5: true, .. } => TextureFormat::Bc5Unorm,
            Self::Hdr => TextureFormat::Rgba16Float,
            Self::Bc6h => TextureFormat::Bc6hUfloat,
        }
    }

    /// Bytes a `size` texture of these takes: the first level's RGBA8 for colors, data and maps a
    /// frame makes levels for; every level's for those that come with them, compressed or HDR.
    fn bytes(self, size: [u32; 2]) -> u64 {
        match self {
            Self::Color(_) | Self::Data | Self::Map(_) => self.format().bytes(size, false),
            Self::ColorLevels
            | Self::MapLevels(_)
            | Self::Bc7 { .. }
            | Self::CompressedMap { .. }
            | Self::Hdr
            | Self::Bc6h => self.format().bytes(size, self.mipmapped()),
        }
    }
}

/// The textures a renderer's shaders sample, the engine's white one first.
///
/// Handles come at once; [`Uploads::take`](crate::Uploads::take) takes pixels for frames to stream within budget, slots
/// showing stand-ins until then. Replacements and removals follow in order. A renderer samples one
/// `Textures`' textures only.
pub struct Textures {
    /// Added, filled, replaced or removed since last taken, in order.
    pub(crate) changes: Vec<Change>,
    /// By index.
    entries: Vec<Entry>,
}

/// A texture handed out.
struct Entry {
    /// What its texels stand for, which replacements keep to.
    kind: Kind,
    /// Its name, size and texels, until its pixels come ([`Textures::fill`]).
    reserved: Option<(String, [u32; 2], Texels)>,
    /// Removed for good.
    removed: bool,
}

/// A change [`Uploads::take`](crate::Uploads::take) carries to a renderer.
pub(crate) enum Change {
    /// A texture added: its slot, showing `kind`'s stand-in until an image arrives.
    Reserved {
        index: u32,
        name: String,
        kind: Kind,
    },
    /// A texture's first image, or a replacement.
    Image(Added),
    /// By index.
    Removed(u32),
}

pub(crate) struct Added {
    pub(crate) index: u32,
    pub(crate) name: String,
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Vec<u8>,
    pub(crate) texels: Texels,
}

impl Textures {
    /// Adds a texture of `size` pixels from `pixels`: RGBA8 rows, tightly packed, top first, or for
    /// compressed texels its levels' blocks ([`Texels::Bc7`]). Colors are sRGB with straight alpha,
    /// sampled as linear; data and maps are read as stored. Levels are made as it streams if
    /// `texels` say so, or come made ([`Texels::ColorLevels`]). A `Vec` is kept; borrowed pixels
    /// are copied. Coordinates run from (0, 0) top left to (1, 1) bottom right; until streamed,
    /// shaders sample its stand-in. `name` names it in errors and GPU captures. It's
    /// [`reserve`](Self::reserve), then [`fill`](Self::fill).
    ///
    /// Fails with `renderer.bad_texture` if a side is 0 or over 8,192 pixels (the most every Vulkan
    /// 1.4 device takes) or `pixels` doesn't fill it exactly; with `renderer.too_many_textures`
    /// past 61,440 textures, the white one included ([`room`](Self::room)).
    pub fn add<'a>(
        &mut self,
        name: &str,
        size: [u32; 2],
        pixels: impl Into<Cow<'a, [u8]>>,
        texels: impl Into<Texels>,
    ) -> Result<Texture> {
        let (pixels, texels) = (pixels.into(), texels.into());
        // Before the handle, so a texture that fails takes none.
        check_pixels(name, size, &pixels, texels)?;
        let texture = self.reserve(name, size, texels)?;
        self.fill(texture, pixels)?;
        Ok(texture)
    }

    /// Hands out a handle for a `size`-pixel texture of `texels` whose pixels come later through
    /// [`fill`](Self::fill), as those decoded off the main thread do; its slot shows its stand-in
    /// until then. `name` names it in errors and GPU captures.
    ///
    /// Fails with `renderer.bad_texture` if a side is 0 or over 8,192 pixels, and with
    /// `renderer.too_many_textures` as [`add`](Self::add) does.
    pub fn reserve(
        &mut self,
        name: &str,
        size: [u32; 2],
        texels: impl Into<Texels>,
    ) -> Result<Texture> {
        let texels = texels.into();
        check_size(name, size)?;
        // Its handle is its slot, so a renderer holds as many as the bindless sets have placed
        // slots.
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
        let kind = texels.kind();
        self.entries.push(Entry {
            kind,
            reserved: Some((name.to_owned(), size, texels)),
            removed: false,
        });
        self.changes.push(Change::Reserved {
            index,
            name: name.to_owned(),
            kind,
        });
        Ok(Texture { index })
    }

    /// Fills `texture`, from [`reserve`](Self::reserve), with `pixels`, as [`add`](Self::add) takes
    /// them for its reserved size and texels. A removed texture stays removed.
    ///
    /// Fails with `renderer.bad_texture` if `pixels` doesn't fill it exactly, leaving it unfilled.
    ///
    /// # Panics
    ///
    /// - If `texture` isn't one of these textures (another `Textures`' handles can be past them).
    /// - If it wasn't reserved, or is filled: another image goes through
    ///   [`replace`](Self::replace).
    pub fn fill<'a>(&mut self, texture: Texture, pixels: impl Into<Cow<'a, [u8]>>) -> Result<()> {
        let Some(entry) = self.entries.get_mut(texture.index as usize) else {
            panic!("texture {} is one of another `Textures`", texture.index);
        };
        if entry.removed {
            return Ok(());
        }
        let Some((name, size, texels)) = &entry.reserved else {
            panic!(
                "texture {} is filled already, or wasn't reserved; another image for it goes \
                 through `replace`",
                texture.index
            );
        };
        let pixels = pixels.into();
        check_pixels(name, *size, &pixels, *texels)?;
        let (name, size, texels) = entry.reserved.take().expect("just there");
        self.changes.push(Change::Image(Added {
            index: texture.index,
            name,
            size,
            pixels: pixels.into_owned(),
            texels,
        }));
        Ok(())
    }

    /// Replaces `texture`'s image, of any `size`, keeping its handle; `pixels` and `texels` as
    /// [`add`](Self::add) takes them. Samplers see the old image until the new one streams. It also
    /// replaces pixels still waiting, or fills a [reserved](Self::reserve) texture, so a later
    /// [`fill`](Self::fill) panics. `name` names it in errors. A removed texture stays removed.
    ///
    /// Fails with `renderer.bad_texture` as `add` does, leaving the texture as it was.
    ///
    /// # Panics
    ///
    /// - If `texture` isn't one of these textures (another `Textures`' handles can be past them).
    /// - If `texels` change its kind, as data for colors or a normal map for a surface map.
    pub fn replace<'a>(
        &mut self,
        name: &str,
        texture: Texture,
        size: [u32; 2],
        pixels: impl Into<Cow<'a, [u8]>>,
        texels: impl Into<Texels>,
    ) -> Result<()> {
        let Some(entry) = self.entries.get_mut(texture.index as usize) else {
            panic!("texture {name:?} replaces one of another `Textures`");
        };
        let texels = texels.into();
        assert_eq!(
            texels.kind(),
            entry.kind,
            "texture {name:?} is replaced by texels of another kind"
        );
        let pixels = pixels.into();
        check_pixels(name, size, &pixels, texels)?;
        if entry.removed {
            return Ok(());
        }
        entry.reserved = None;
        let pixels = pixels.into_owned();
        let waiting = (self.changes.iter_mut()).find_map(|change| match change {
            Change::Image(added) if added.index == texture.index => Some(added),
            _ => None,
        });
        match waiting {
            Some(waiting) => {
                waiting.size = size;
                waiting.pixels = pixels;
                waiting.texels = texels;
            }
            None => self.changes.push(Change::Image(Added {
                index: texture.index,
                name: name.to_owned(),
                size,
                pixels,
                texels,
            })),
        }
        Ok(())
    }

    /// Removes `texture`: from the frame taking the removal, its slot shows its stand-in for good,
    /// as stale handles (in worlds or snapshots) may sample it. Unstreamed pixels are dropped; its
    /// image goes once the GPU is done with it. Its handle is never reused; removing it again does
    /// nothing.
    ///
    /// # Panics
    ///
    /// - If `texture` isn't one of these textures (another `Textures`' handles can be past them).
    pub fn remove(&mut self, texture: Texture) {
        let Some(entry) = self.entries.get_mut(texture.index as usize) else {
            panic!("texture {} is one of another `Textures`", texture.index);
        };
        if entry.removed {
            return;
        }
        entry.removed = true;
        entry.reserved = None;
        self.changes.push(Change::Removed(texture.index));
    }

    /// How many more textures it holds, so a caller adding several can check they fit first.
    pub fn room(&self) -> u32 {
        PLACED_TEXTURES - self.count()
    }

    /// How many it has handed out, the white texture among them: the next index.
    pub fn count(&self) -> u32 {
        self.entries.len() as u32
    }
}

/// The error for texture `name`, with `problem` a phrase following its name.
fn bad_texture(name: &str, problem: String) -> Error {
    Error::new(
        "renderer.bad_texture",
        format!("texture {name:?} {problem}"),
    )
}

/// Checks `size` is one every Vulkan 1.4 device takes ([`Textures::add`]).
fn check_size(name: &str, size: [u32; 2]) -> Result<()> {
    let [width, height] = size;
    let sides = 1..=MAX_TEXTURE_SIZE;
    if !sides.contains(&width) || !sides.contains(&height) {
        return Err(bad_texture(
            name,
            format!(
                "is {width}x{height} pixels, but textures are 1 to {MAX_TEXTURE_SIZE} pixels a side"
            ),
        ));
    }
    Ok(())
}

/// Checks `pixels` fill a texture of `texels` and `size` that every Vulkan 1.4 device takes
/// ([`Textures::add`]).
fn check_pixels(name: &str, size: [u32; 2], pixels: &[u8], texels: Texels) -> Result<()> {
    check_size(name, size)?;
    let [width, height] = size;
    let bytes = texels.bytes(size);
    if pixels.len() as u64 != bytes {
        let of = match texels {
            Texels::Color(_) | Texels::Data | Texels::Map(_) => "RGBA8",
            Texels::ColorLevels | Texels::MapLevels(_) => "RGBA8 in its levels",
            Texels::Bc7 { mipmapped: false }
            | Texels::CompressedMap {
                bc5: false,
                mipmapped: false,
                ..
            } => "BC7 blocks",
            Texels::Bc7 { mipmapped: true }
            | Texels::CompressedMap {
                bc5: false,
                mipmapped: true,
                ..
            } => "BC7 blocks in its levels",
            Texels::CompressedMap {
                bc5: true,
                mipmapped: false,
                ..
            } => "BC5 blocks",
            Texels::CompressedMap {
                bc5: true,
                mipmapped: true,
                ..
            } => "BC5 blocks in its levels",
            Texels::Hdr => "RGBA of 16-bit floats",
            Texels::Bc6h => "BC6H blocks",
        };
        return Err(bad_texture(
            name,
            format!(
                "is {width}x{height} pixels, {bytes} bytes of {of}, but has {} bytes",
                pixels.len()
            ),
        ));
    }
    Ok(())
}

/// Appends to `pixels` (a `size` texture's RGBA8, sRGB, straight alpha) the rest of its mip chain
/// as `mipmaps` says, smallest last. Each level area-averages the last in linear space, as shaders
/// filter. Levels stay floats until encoded, so each comes from the first's exact colors; the first
/// stays bytes, as floats take four times the memory (16 MiB for a 2,048 by 2,048 texture's second
/// level). The cooker makes levels with it before compressing; frames make them for textures it
/// didn't.
#[doc(hidden)]
pub fn add_mipmaps(pixels: &mut Vec<u8>, size: [u32; 2], mipmaps: Mipmaps) {
    let weighted = match mipmaps {
        Mipmaps::None => return,
        Mipmaps::Averaged => false,
        Mipmaps::AlphaWeighted => true,
    };
    let _zone = gg_core::zone!();
    let decode: [f32; 256] = array::from_fn(|byte| srgb_to_linear(byte as f32 / 255.0));
    let encoder = Encoder::new();
    let encode = |linear| encoder.encode(linear);
    let append = |pixels: &mut Vec<u8>, level: &[[f32; 4]]| {
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
    reserve_levels(pixels, from);
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

/// As [`add_mipmaps`], for a `size` map's RGBA8, but averaging channels as stored, not decoded from
/// sRGB, alpha alike. A normal map's averaged X and Y shorten the normal where those below differ,
/// as filtering does; shaders make it whole again.
#[doc(hidden)]
pub fn add_map_mipmaps(pixels: &mut Vec<u8>, size: [u32; 2]) {
    let _zone = gg_core::zone!();
    // Channels written out, as arrays' `map` takes several times as long in debug builds.
    let append = |pixels: &mut Vec<u8>, level: &[[f32; 4]]| {
        let byte = |value: f32| (value * 255.0).round() as u8;
        for &[r, g, b, a] in level {
            pixels.extend_from_slice(&[byte(r), byte(g), byte(b), byte(a)]);
        }
    };
    let mut sizes = mip_sizes(size[0], size[1]);
    let first_size = sizes.next().expect("a chain has its first level");
    let Some(mut from) = sizes.next() else {
        return;
    };
    reserve_levels(pixels, from);
    let first = pixels.as_chunks::<4>().0;
    let texel = |i: usize| {
        let [r, g, b, a] = first[i];
        let value = |byte| f32::from(byte) / 255.0;
        [value(r), value(g), value(b), value(a)]
    };
    let mut level = shrink(texel, first_size, from, false);
    append(pixels, &level);
    for to in sizes {
        level = shrink(|i| level[i], from, to, false);
        append(pixels, &level);
        from = to;
    }
}

/// Reserves `pixels`' RGBA8 for every level from one of `from` texels on, at once: growing level by
/// level would double the first's room, a 4,096 by 4,096 texture's 64 MiB to 128, rather than the
/// 85 its chain takes.
fn reserve_levels(pixels: &mut Vec<u8>, [width, height]: [u32; 2]) {
    let levels = mip_sizes(width, height);
    pixels.reserve_exact(
        levels
            .map(|[width, height]| width as usize * height as usize * 4)
            .sum(),
    );
}

/// Appends to `pixels`, a `size` texture's RGBA8 of `texels`, the levels a frame would make: a
/// color's by [`add_mipmaps`], a map's by [`add_map_mipmaps`], none for what comes with levels or
/// has none. The pixels are then [`Texels::with_levels`]'.
#[doc(hidden)]
pub fn add_levels(pixels: &mut Vec<u8>, size: [u32; 2], texels: Texels) {
    match texels {
        Texels::Color(mipmaps) => add_mipmaps(pixels, size, mipmaps),
        Texels::Map(_) => add_map_mipmaps(pixels, size),
        Texels::ColorLevels
        | Texels::Data
        | Texels::Bc7 { .. }
        | Texels::MapLevels(_)
        | Texels::CompressedMap { .. }
        | Texels::Hdr
        | Texels::Bc6h => {}
    }
}

/// The next mip level, `to` texels, of a `from`-texel level whose `i`th texel is `texel(i)` (linear
/// straight RGB, then alpha). Each texel area-averages those it covers, colors weighted by alpha if
/// `weighted` (plainly if all are transparent). Loops are written out: through iterators, a 2,048
/// by 2,048 texture's levels took seconds in debug builds.
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
                [color[0] / weights, color[1] / weights, color[2] / weights]
            } else {
                plain
            };
            level.push([r, g, b, alpha]);
        }
    }
    level
}

/// The texels along a side of `from` that the next level's texel `i` covers, with weights adding to
/// 1, then how many there are. Even `from`: two halves. Odd `2m + 1`: the next level has `m`, each
/// covering `from / m` texels, so three, the outer two in part. 1: one, as the next level is 1 too.
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

/// Encodes linear channels, 0 to 1, as sRGB bytes, rounding as GPUs do to the byte nearest once
/// encoded, without a power per channel.
struct Encoder {
    /// The linear values where encoding rounds up to each byte from the one below, then NaN, which
    /// none reach.
    rounding: [f32; 256],
    /// The byte at the start of each [`STEPS`](Self::STEPS)th of the way from 0 to 1.
    below: [u8; Self::STEPS + 1],
}

impl Encoder {
    /// How finely `below` divides 0 to 1: finer than bytes come at their closest, at 0 (12.92 ×
    /// 255, about 3,295 over the way). So each step holds at most one rounding point, and one
    /// comparison finds it, as a search of `rounding` would, without its mispredicting branches and
    /// far faster in debug builds.
    const STEPS: usize = 4096;

    fn new() -> Self {
        let rounding = array::from_fn(|byte| {
            if byte < 255 {
                srgb_to_linear((byte as f32 + 0.5) / 255.0)
            } else {
                f32::NAN
            }
        });
        let mut below = [0; Self::STEPS + 1];
        let mut byte = 0;
        for (step, below) in below.iter_mut().enumerate() {
            while rounding[byte] <= step as f32 / Self::STEPS as f32 {
                byte += 1;
            }
            *below = byte as u8;
        }
        Self { rounding, below }
    }

    /// `linear`, encoded: past 1 as 1, below 0 or NaN as 0.
    fn encode(&self, linear: f32) -> u8 {
        // Exact, as scaling by a power of two is, and saturating.
        let step = ((linear * Self::STEPS as f32) as usize).min(Self::STEPS);
        let byte = self.below[step];
        byte + u8::from(self.rounding[usize::from(byte)] <= linear)
    }
}

/// An sRGB channel, 0 to 1, decoded to linear as Vulkan's sRGB formats do.
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
            changes: Vec::new(),
            entries: Vec::new(),
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

    /// The changes waiting: each slot by index and kind, each image by texture index, size and
    /// texels, and each removal.
    #[derive(Clone, Debug, PartialEq)]
    enum Waiting {
        Slot(u32, Kind),
        Image(u32, [u32; 2], Texels),
        Removed(u32),
    }

    fn waiting(textures: &Textures) -> Vec<Waiting> {
        (textures.changes.iter())
            .map(|change| match change {
                Change::Reserved { index, kind, .. } => Waiting::Slot(*index, *kind),
                Change::Image(added) => Waiting::Image(added.index, added.size, added.texels),
                Change::Removed(index) => Waiting::Removed(*index),
            })
            .collect()
    }

    /// The images waiting, in order.
    fn images(textures: &Textures) -> Vec<&Added> {
        (textures.changes.iter())
            .filter_map(|change| match change {
                Change::Image(added) => Some(added),
                Change::Reserved { .. } | Change::Removed(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_texture_replaced_keeps_its_handle_and_the_place_of_pixels_waiting() {
        use Waiting::{Image, Slot};
        let mut textures = Textures::default();
        let a = textures
            .add("a", [1, 1], &[0; 4], Mipmaps::None)
            .expect("a");
        // Still waiting as added, so the replacement takes its place.
        let replaced = textures.replace("a", a, [2, 1], &[9; 8], Mipmaps::Averaged);
        replaced.expect("another image");
        let none = Texels::Color(Mipmaps::None);
        let averaged = Texels::Color(Mipmaps::Averaged);
        let white = [Slot(0, Kind::Color), Image(0, [1, 1], none)];
        let added = [Slot(1, Kind::Color), Image(1, [2, 1], averaged)];
        assert_eq!(waiting(&textures), [white, added].concat());
        // Once a frame has taken it, in place of the renderer's.
        textures.changes.clear();
        let replaced = textures.replace("a", a, [1, 2], &[7; 8], Mipmaps::None);
        replaced.expect("another image");
        assert_eq!(waiting(&textures), [Image(1, [1, 2], none)]);
        // A broken image leaves it as it was.
        let error = textures
            .replace("a", a, [2, 2], &[0; 4], Mipmaps::None)
            .expect_err("a broken image");
        assert_eq!(error.code(), "renderer.bad_texture");
        assert_eq!(waiting(&textures), [Image(1, [1, 2], none)]);
        assert_eq!(images(&textures)[0].pixels, [7; 8]);
        // Data, replaced by data.
        let field = textures.add("field", [1, 1], &[0; 4], Texels::Data);
        let field = field.expect("data");
        let replaced = textures.replace("field", field, [2, 1], &[1; 8], Texels::Data);
        replaced.expect("other data");
        let data = [Slot(2, Kind::Data), Image(2, [2, 1], Texels::Data)];
        assert_eq!(
            waiting(&textures),
            [vec![Image(1, [1, 2], none)], data.into()].concat()
        );
    }

    /// A reserved texture has its slot at once and its image once filled, checked as `add` checks.
    /// Filling it twice, or after `add`, is a bug: replace instead. A removal follows earlier
    /// changes once, however often; its handle isn't reused, and later pixels for it are checked,
    /// then dropped.
    #[test]
    fn a_texture_reserved_is_filled_once_and_one_removed_stays_removed_in_order() {
        use Waiting::{Image, Removed, Slot};
        let mut textures = Textures::default();
        textures.changes.clear();
        let field = textures
            .reserve("field", [2, 1], Texels::Data)
            .expect("a handle");
        assert_eq!(field.index, 1);
        assert_eq!(waiting(&textures), [Slot(1, Kind::Data)]);
        let error = textures.fill(field, &[0; 4]).expect_err("too few pixels");
        assert_eq!(error.code(), "renderer.bad_texture");
        assert!(error.to_string().contains("\"field\""), "{error}");
        textures.fill(field, &[0; 8]).expect("its pixels");
        assert_eq!(
            waiting(&textures),
            [Slot(1, Kind::Data), Image(1, [2, 1], Texels::Data)]
        );
        let error = textures.reserve("huge", [MAX_TEXTURE_SIZE + 1, 1], Mipmaps::None);
        assert_eq!(error.expect_err("too wide").code(), "renderer.bad_texture");
        let message = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = textures.fill(field, &[0; 8]);
        }));
        assert!(message.is_err(), "filled twice");
        let message = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = textures.fill(Texture::WHITE, &[0; 4]);
        }));
        assert!(message.is_err(), "added, so filled already");

        textures.changes.clear();
        let a = textures
            .reserve("a", [1, 1], Mipmaps::None)
            .expect("a handle");
        textures.remove(a);
        textures.remove(a);
        textures.fill(a, &[0; 4]).expect("checked, then dropped");
        let b = textures
            .add("b", [1, 1], &[0; 4], Mipmaps::None)
            .expect("b");
        assert_eq!((a.index, b.index), (2, 3));
        let none = Texels::Color(Mipmaps::None);
        let expected = [
            Slot(2, Kind::Color),
            Removed(2),
            Slot(3, Kind::Color),
            Image(3, [1, 1], none),
        ];
        assert_eq!(waiting(&textures), expected);
        textures.changes.clear();
        textures
            .replace("a", a, [1, 1], &[0; 4], Mipmaps::None)
            .expect("checked");
        assert!(textures.changes.is_empty(), "dropped");
        let error = textures.replace("a", a, [1, 1], &[0; 3], Mipmaps::None);
        assert_eq!(
            error.expect_err("too few pixels").code(),
            "renderer.bad_texture"
        );
        assert_eq!(textures.room(), PLACED_TEXTURES - 4);
    }

    #[test]
    #[should_panic(expected = "is one of another `Textures`")]
    fn another_textures_texture_cant_be_removed() {
        Textures::default().remove(Texture { index: 5 });
    }

    /// A replacement before a reserved texture's pixels come fills it, so the pixels coming after
    /// are a bug. Colors with levels made already are filled by every level's RGBA8.
    #[test]
    fn a_reserved_texture_replaced_is_filled_and_one_with_levels_takes_them_all() {
        use Waiting::{Image, Slot};
        let mut textures = Textures::default();
        textures.changes.clear();
        let a = textures.reserve("a", [2, 2], Texels::ColorLevels);
        let a = a.expect("a handle");
        let replaced = textures.replace("a", a, [1, 1], &[0; 4], Mipmaps::None);
        replaced.expect("another image");
        let none = Texels::Color(Mipmaps::None);
        assert_eq!(
            waiting(&textures),
            [Slot(1, Kind::Color), Image(1, [1, 1], none)]
        );
        let late = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = textures.fill(a, &[0; 20]);
        }));
        assert!(late.is_err(), "filled by the replacement");
        // 2x2 texels, then its 1x1 level.
        let b = textures.reserve("b", [2, 2], Texels::ColorLevels);
        let b = b.expect("a handle");
        let error = textures
            .fill(b, &[0; 16])
            .expect_err("its first level alone");
        assert!(error.to_string().contains("RGBA8 in its levels"), "{error}");
        textures.fill(b, &[0; 20]).expect("every level");
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
        // Colors take compressed colors in their place, and back.
        let replaced = textures.replace("one", texture, [2, 2], &[0; 16], Mipmaps::None);
        replaced.expect("RGBA8 in place of BC7");
        let replaced = textures.replace("one", texture, [1, 1], blocks(1), chain);
        replaced.expect("BC7 in place of RGBA8");
        // And HDR colors, stored or compressed, a level each.
        let replaced = textures.replace("one", texture, [2, 1], &[0; 16], Texels::Hdr);
        replaced.expect("16-bit floats in place of BC7");
        let replaced = textures.replace("one", texture, [5, 5], blocks(4), Texels::Bc6h);
        replaced.expect("BC6H in place of 16-bit floats");
        let error = (textures.add("short", [2, 1], &[0; 8], Texels::Hdr))
            .expect_err("RGBA8's bytes where 16-bit floats are twice them");
        assert!(error.to_string().contains("16-bit floats"), "{error}");
    }

    #[test]
    #[should_panic(expected = "replaced by texels of another kind")]
    fn a_texture_keeps_its_kind_of_texels() {
        let mut textures = Textures::default();
        let field = textures.add("field", [1, 1], &[0; 4], Texels::Data);
        let field = field.expect("data");
        let _ = textures.replace("field", field, [1, 1], &[0; 4], Mipmaps::None);
    }

    #[test]
    #[should_panic(expected = "replaced by texels of another kind")]
    fn a_normal_map_stays_a_normal_map() {
        let mut textures = Textures::default();
        let normals = textures.add("n", [1, 1], &[0; 4], Texels::Map(Map::Normal));
        let normals = normals.expect("a normal map");
        let surface = Texels::Map(Map::Surface);
        let _ = textures.replace("n", normals, [1, 1], &[0; 4], surface);
    }

    /// A map's levels average values as stored, a color's decoded from sRGB, so halfway between 0
    /// and 255 is 128, not 188; and a map's texels say, once made, that its levels are there.
    #[test]
    fn a_maps_levels_average_its_values_as_stored() {
        let pixels = [0, 0, 0, 0, 255, 255, 255, 255];
        let mut map = pixels.to_vec();
        add_levels(&mut map, [2, 1], Texels::Map(Map::Normal));
        assert_eq!(map.split_off(8), [128; 4]);
        let color = levels(&pixels, [2, 1], Mipmaps::Averaged);
        assert_eq!(color, [188, 188, 188, 128]);
        assert_eq!(
            Texels::Map(Map::Surface).with_levels(),
            Texels::MapLevels(Map::Surface)
        );
        let surface = Texels::CompressedMap {
            map: Map::Surface,
            bc5: false,
            mipmapped: true,
        };
        assert_eq!(surface.with_levels(), surface);
        assert_eq!(surface.format(), TextureFormat::Bc7Unorm);
        let normals = Texels::CompressedMap {
            map: Map::Normal,
            bc5: true,
            mipmapped: false,
        };
        assert_eq!(normals.format(), TextureFormat::Bc5Unorm);
        assert_eq!(normals.bytes([5, 5]), 4 * 16, "four blocks");
        // A normal map as a DDS image holds one, X, Y and Z, as BC7.
        let bc7_normals = Texels::CompressedMap {
            map: Map::Normal,
            bc5: false,
            mipmapped: false,
        };
        assert_eq!(bc7_normals.format(), TextureFormat::Bc7Unorm);
        assert_eq!(bc7_normals.kind(), Kind::Map(Map::Normal));
    }

    /// The levels after the first that `pixels`, of `size`, gets.
    fn levels(pixels: &[u8], size: [u32; 2], mipmaps: Mipmaps) -> Vec<u8> {
        let mut levels = pixels.to_vec();
        add_mipmaps(&mut levels, size, mipmaps);
        levels.split_off(pixels.len())
    }

    #[test]
    fn levels_average_in_linear_space_by_area() {
        use Mipmaps::{AlphaWeighted, Averaged};
        let (white, black) = ([255; 4], [0, 0, 0, 255]);
        // Half white light is 188 encoded; averaging encoded bytes would give 128.
        let checker = [white, black, black, white].concat();
        assert_eq!(levels(&checker, [2, 2], Averaged), [188, 188, 188, 255]);
        // Three texels into one: a third each, none skipped.
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

    /// Each byte's decoded value encodes as that byte. Encoding rounds up to each byte exactly
    /// halfway from the byte below, as a search would, not a float before; past 1, below 0 or NaN
    /// takes the nearest end.
    #[test]
    fn colors_encode_as_the_nearest_byte() {
        let encoder = Encoder::new();
        for byte in 0..=255 {
            let linear = srgb_to_linear(f32::from(byte) / 255.0);
            assert_eq!(encoder.encode(linear), byte, "byte {byte}");
        }
        for byte in 0..255 {
            let up = encoder.rounding[usize::from(byte)];
            assert_eq!(encoder.encode(up), byte + 1, "at {up}");
            assert_eq!(encoder.encode(up.next_down()), byte, "just below {up}");
        }
        for (linear, byte) in [(2.0, 255), (f32::INFINITY, 255), (-1.0, 0), (f32::NAN, 0)] {
            assert_eq!(encoder.encode(linear), byte, "{linear}");
        }
    }

    #[test]
    fn alpha_weighted_levels_keep_transparent_colors_out() {
        use Mipmaps::{AlphaWeighted, Averaged};
        let (red, clear_green) = ([255, 0, 0, 255], [0, 255, 0, 0]);
        let pair = [red, clear_green].concat();
        assert_eq!(levels(&pair, [2, 1], AlphaWeighted), [255, 0, 0, 128]);
        assert_eq!(levels(&pair, [2, 1], Averaged), [188, 188, 0, 128]);
        // All transparent: colors average plainly rather than dividing by 0.
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
