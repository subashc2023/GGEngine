//! Images that layers load as textures, and models' images. Each is read as far as its header
//! (which gives its size) when added, and decoded later by the loader, off the main thread:
//!
//! - PNGs and JPEGs decode to RGBA8.
//! - Radiance `.hdr` files (high dynamic range, such as skies) decode to RGBA of 16-bit floats.
//! - Models' BC7 DDS images, named by `MSFT_texture_dds`, have their blocks taken out.
//! - A game's package holds BC7, BC5 and BC6H in KTX2, as the cooker compressed them; the blocks
//!   come out of any supercompression.

use std::io::Cursor;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use gg_core::{Error, Result};
use gg_renderer::{MAX_TEXTURE_SIZE, Map, Texels, mip_sizes};
use half::f16;
use image::ImageDecoder;
use image::codecs::hdr::HdrDecoder;
use zune_jpeg::JpegDecoder;
use zune_jpeg::errors::DecodeErrors;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::package::{Files, Kind};

/// The most bytes an image's file may hold: above a texture-sized PNG or JPEG's need (8,192 pixels
/// square of 16-bit RGBA, uncompressed, is 512 MiB).
pub(crate) const MAX_FILE: u64 = 1 << 30;

/// An image's size in pixels and its pixels, as its format says: colors sRGB-encoded, alpha not
/// multiplied in.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub size: [u32; 2],
    pub pixels: Vec<u8>,
    pub format: Format,
}

/// What an [`Image`]'s pixels are: decoded, or a file read as far as its header, for the loader to
/// decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Tightly packed RGBA8 rows, top row first, one level.
    Rgba8,
    /// Tightly packed rows of linear 16-bit-float RGBA, 8 bytes a pixel, top row first, one level,
    /// alpha 1. [`Texels::Hdr`] takes them.
    Rgba16f,
    /// Blocks of 4 by 4 pixels, 16 bytes each, compressed as `compressed` says, as a game's package
    /// holds a texture: the first level or, if `mipmapped`, the cooker's full chain.
    /// [`Texels::Bc7`] takes them.
    Bc {
        compressed: Compressed,
        mipmapped: bool,
    },
    /// A PNG file, header read.
    Png,
    /// A JPEG file, headers read, reaching its end.
    Jpeg,
    /// A Radiance HDR file (`.hdr`), header read.
    Hdr,
    /// A KTX2 file as the cooker writes one, header and level lengths read: blocks compressed as
    /// `compressed` says, the first level or, if `mipmapped`, the full chain, each level zstd if
    /// `zstd`. Decodes to the levels' blocks.
    Ktx2 {
        compressed: Compressed,
        mipmapped: bool,
        zstd: bool,
    },
    /// A model's `MSFT_texture_dds` DDS file, headers read: BC7 blocks, sRGB or values as
    /// `compressed` says, the first level or, if `mipmapped`, the full chain. Decodes to the
    /// levels' blocks as they are.
    Dds {
        compressed: Compressed,
        mipmapped: bool,
    },
}

/// How an image's blocks are compressed: as the cooker compressed them, or as a model's DDS image
/// holds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compressed {
    /// BC7 of sRGB-encoded colors: textures, and models' base colors and emission.
    Bc7Srgb,
    /// BC7 of values as stored: a model's metallic-roughness and occlusion maps.
    Bc7Unorm,
    /// BC5 of a normal map's X and Y.
    Bc5,
    /// Unsigned BC6H of colors of high dynamic range, as skies are, one level.
    Bc6h,
}

impl Compressed {
    const ALL: [Self; 4] = [Self::Bc7Srgb, Self::Bc7Unorm, Self::Bc5, Self::Bc6h];

    /// How the cooker compresses `texels`: colors as sRGB BC7, a surface map as BC7 of values, a
    /// normal map as BC5, high-dynamic-range colors as BC6H.
    pub fn of(texels: Texels) -> Self {
        match texels {
            Texels::Map(Map::Surface)
            | Texels::MapLevels(Map::Surface)
            | Texels::CompressedMap {
                map: Map::Surface, ..
            } => Self::Bc7Unorm,
            Texels::Map(Map::Normal)
            | Texels::MapLevels(Map::Normal)
            | Texels::CompressedMap {
                map: Map::Normal, ..
            } => Self::Bc5,
            Texels::Color(_) | Texels::ColorLevels | Texels::Data | Texels::Bc7 { .. } => {
                Self::Bc7Srgb
            }
            Texels::Hdr | Texels::Bc6h => Self::Bc6h,
        }
    }

    /// Whether a model's image of `texels` may be compressed so in a game's package: as
    /// [`of`](Self::of) says, or, for a normal map, BC7 of values (the cooker keeps a DDS image's
    /// blocks).
    pub(crate) fn holds(self, texels: Texels) -> bool {
        let normals = matches!(Compressed::of(texels), Self::Bc5);
        self == Self::of(texels) || (normals && self == Self::Bc7Unorm)
    }

    pub(crate) fn ktx2(self) -> ktx2::Format {
        match self {
            Self::Bc7Srgb => ktx2::Format::BC7_SRGB_BLOCK,
            Self::Bc7Unorm => ktx2::Format::BC7_UNORM_BLOCK,
            Self::Bc5 => ktx2::Format::BC5_UNORM_BLOCK,
            Self::Bc6h => ktx2::Format::BC6H_UFLOAT_BLOCK,
        }
    }
}

impl Image {
    /// What a texture of it holds. RGBA8 or what decodes to it: `texels`, a [`Texels::Color`] or
    /// [`Texels::Map`], with smaller levels made per frame. A Radiance HDR: high-dynamic-range
    /// colors, one level. A compressed image: its cooked or stored levels, BC7 as colors or maps
    /// per `texels`, sRGB or values alike (the blocks are the same).
    pub(crate) fn texels(&self, texels: Texels) -> Texels {
        let (compressed, mipmapped) = match self.format {
            Format::Rgba8 | Format::Png | Format::Jpeg => return texels,
            Format::Rgba16f | Format::Hdr => return Texels::Hdr,
            Format::Bc {
                compressed,
                mipmapped,
            }
            | Format::Ktx2 {
                compressed,
                mipmapped,
                ..
            }
            | Format::Dds {
                compressed,
                mipmapped,
            } => (compressed, mipmapped),
        };
        let map = match texels {
            Texels::Map(map) | Texels::MapLevels(map) | Texels::CompressedMap { map, .. } => {
                Some(map)
            }
            _ => None,
        };
        match (compressed, map) {
            (Compressed::Bc6h, _) => Texels::Bc6h,
            // BC5 holds a normal map's X and Y alone.
            (Compressed::Bc5, map) => Texels::CompressedMap {
                map: map.unwrap_or(Map::Normal),
                bc5: true,
                mipmapped,
            },
            (Compressed::Bc7Srgb | Compressed::Bc7Unorm, Some(map)) => Texels::CompressedMap {
                map,
                bc5: false,
                mipmapped,
            },
            (Compressed::Bc7Srgb | Compressed::Bc7Unorm, None) => Texels::Bc7 { mipmapped },
        }
    }

    pub(crate) fn compressed(&self) -> Option<Compressed> {
        match self.format {
            Format::Rgba8 | Format::Rgba16f | Format::Png | Format::Jpeg | Format::Hdr => None,
            Format::Bc { compressed, .. }
            | Format::Ktx2 { compressed, .. }
            | Format::Dds { compressed, .. } => Some(compressed),
        }
    }

    /// Bytes its pixels take decoded: RGBA8 or 16-bit-float RGBA of its size, or each level's
    /// blocks. Reading a model counts what it will hold.
    pub(crate) fn decoded_len(&self) -> usize {
        let [width, height] = self.size.map(|side| side as usize);
        match self.format {
            Format::Rgba8 | Format::Png | Format::Jpeg => width * height * 4,
            Format::Rgba16f | Format::Hdr => width * height * 8,
            Format::Bc { mipmapped, .. }
            | Format::Ktx2 { mipmapped, .. }
            | Format::Dds { mipmapped, .. } => {
                let levels = if mipmapped { usize::MAX } else { 1 };
                let [width, height] = self.size;
                mip_sizes(width, height).take(levels).map(blocks).sum()
            }
        }
    }

    /// It decoded: a PNG or JPEG to RGBA8 (see [`read`]), a KTX2 or DDS file to its levels' blocks
    /// (out of zstd where supercompressed); itself if already decoded. The error says what's wrong.
    ///
    /// Fails for a file whose pixels don't decode (cut short or corrupt past the header), or that
    /// decodes to another size than its header said.
    pub(crate) fn decoded(self) -> Result<Image, String> {
        let decode: fn(&[u8]) -> Result<Image, String> = match self.format {
            Format::Rgba8 | Format::Rgba16f | Format::Bc { .. } => return Ok(self),
            Format::Png => decode_png,
            Format::Jpeg => decode_jpeg,
            Format::Hdr => decode_hdr,
            Format::Ktx2 { .. } => decode_ktx2_levels,
            Format::Dds { .. } => decode_dds,
        };
        // As the header's read is, in `read` and `read_ktx2`.
        let decoded = panic::catch_unwind(|| decode(&self.pixels))
            .unwrap_or_else(|_| Err("its decoder panicked on it".to_owned()))?;
        if decoded.size != self.size {
            let [[width, height], [was_width, was_height]] = [decoded.size, self.size];
            return Err(format!(
                "it decodes to {width}x{height} pixels, where its header says {was_width}x\
                 {was_height}"
            ));
        }
        Ok(decoded)
    }
}

/// Bytes of a level's blocks (4 by 4 texels, 16 bytes each, covering the edges).
fn blocks([width, height]: [u32; 2]) -> usize {
    (width.div_ceil(4) as usize) * (height.div_ceil(4) as usize) * 16
}

/// Reads texture `name`'s image at `path` from `files` as far as its header: a PNG, JPEG or
/// Radiance HDR on disk ([`read`], [`read_hdr`]), or the cooker's texture in the game's package
/// ([`read_ktx2`]). [`decode_texture`] decodes it.
///
/// Fails with `engine.bad_texture` if the file can't be read, isn't a regular file, or doesn't
/// read as any of those; or if the package lacks it as a texture or holds one not written as the
/// cooker writes colors.
pub(crate) fn load_image(name: &str, path: &Path, files: &Files) -> Result<Image> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bad = |problem: String| {
        Error::new(
            "engine.bad_texture",
            format!("texture {name:?} at {} {problem}", path.display()),
        )
    };
    // One byte past the most, so a longer file fails to read.
    let bytes = (files.read_as(path, Kind::Texture, MAX_FILE + 1))
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    if files.packaged().is_some() {
        let read = read_ktx2(bytes).and_then(|image| match image.compressed() {
            Some(Compressed::Bc7Srgb | Compressed::Bc6h) => Ok(image),
            other => Err(format!("it holds {other:?}, where a texture holds colors")),
        });
        return read.map_err(|problem| bad(format!("doesn't read as a cooked texture: {problem}")));
    }
    if is_hdr(&bytes) {
        return read_hdr(bytes)
            .map_err(|problem| bad(format!("doesn't decode as a Radiance HDR: {problem}")));
    }
    read(bytes).map_err(|problem| bad(format!("doesn't decode as a PNG or a JPEG: {problem}")))
}

/// Decodes texture `name`'s `image` from `path`, as [`load_image`] read it.
///
/// Fails with `engine.bad_texture`, naming texture and file, if it doesn't decode.
pub(crate) fn decode_texture(name: &str, path: &Path, image: Image) -> Result<Image> {
    let what = match image.format {
        Format::Ktx2 { .. } | Format::Bc { .. } => "doesn't read as a cooked texture",
        Format::Dds { .. } => "doesn't read as a DDS of BC7",
        Format::Png | Format::Jpeg | Format::Rgba8 => "doesn't decode as a PNG or a JPEG",
        Format::Hdr | Format::Rgba16f => "doesn't decode as a Radiance HDR",
    };
    image.decoded().map_err(|problem| {
        let message = format!("texture {name:?} at {} {what}: {problem}", path.display());
        Error::new("engine.bad_texture", message)
    })
}

/// Reads and decodes a texture as the cooker writes it, into a game's package or a model it cooked.
///
/// It is KTX2 of what [`Compressed`] says, a 2D image of one level or the full chain [`mip_sizes`]
/// gives, each level plain or zstd-supercompressed. The error says what's wrong with it.
pub fn decode_ktx2(bytes: &[u8]) -> Result<Image, String> {
    read_ktx2(bytes.to_vec())?.decoded()
}

/// Reads a [`decode_ktx2`] texture's header and level lengths, keeping `bytes` for
/// [`Image::decoded`].
///
/// Fails before allocating for pixels if a side is 0 or over [`MAX_TEXTURE_SIZE`].
pub(crate) fn read_ktx2(bytes: Vec<u8>) -> Result<Image, String> {
    // ktx2 (0.5.0) checks a file's sections against its length, but panics on a data format
    // descriptor giving a block's side as 256: the file's doing, as with zune-jpeg's panics below.
    let reader = match panic::catch_unwind(|| ktx2::Reader::new(&bytes[..])) {
        Ok(Ok(reader)) => reader,
        Ok(Err(error)) => return Err(format!("it isn't KTX2: {error}")),
        Err(_) => return Err("its reader panicked on it".to_owned()),
    };
    let header = reader.header();
    let compressed =
        (Compressed::ALL.into_iter()).find(|compressed| header.format == Some(compressed.ktx2()));
    let Some(compressed) = compressed else {
        return Err(format!(
            "it holds {:?}, not BC7 of sRGB or of values, BC5 or BC6H",
            header.format
        ));
    };
    if header.pixel_depth != 0 || header.layer_count != 0 || header.face_count != 1 {
        return Err("it isn't one 2D image".to_owned());
    }
    let size = [header.pixel_width, header.pixel_height];
    check_size(size[0], size[1])?;
    let chain = mip_sizes(size[0], size[1]).count() as u32;
    let mipmapped = match header.level_count {
        1 => false,
        count if count == chain => true,
        count => {
            return Err(format!(
                "it has {count} levels, neither 1 nor the {chain} of its full chain"
            ));
        }
    };
    let zstd = match header.supercompression_scheme {
        None => false,
        Some(ktx2::SupercompressionScheme::Zstandard) => true,
        Some(other) => return Err(format!("its levels are supercompressed by {other:?}")),
    };
    let sizes = mip_sizes(size[0], size[1]).take(reader.levels().len());
    for (i, (level, size)) in reader.levels().zip(sizes).enumerate() {
        let len = blocks(size);
        if level.uncompressed_byte_length != len as u64 {
            return Err(format!(
                "its level {i} holds {} bytes, not the {len} of its blocks",
                level.uncompressed_byte_length
            ));
        }
        if !zstd && level.data.len() != len {
            return Err(format!(
                "its level {i} has {} bytes, not the {len} of its blocks",
                level.data.len()
            ));
        }
    }
    Ok(Image {
        size,
        pixels: bytes,
        format: Format::Ktx2 {
            compressed,
            mipmapped,
            zstd,
        },
    })
}

fn decode_ktx2_levels(bytes: &[u8]) -> Result<Image, String> {
    let reader = ktx2::Reader::new(bytes).map_err(|error| format!("it isn't KTX2: {error}"))?;
    let header = reader.header();
    let size = [header.pixel_width, header.pixel_height];
    // What `read_ktx2` read it to hold.
    let compressed = (Compressed::ALL.into_iter())
        .find(|compressed| header.format == Some(compressed.ktx2()))
        .unwrap_or(Compressed::Bc7Srgb);
    // One level or the full chain, as read; zstd is the only supercompression read accepts.
    let (mipmapped, zstd) = (
        header.level_count > 1,
        header.supercompression_scheme.is_some(),
    );
    let sizes = || mip_sizes(size[0], size[1]).take(reader.levels().len());
    let mut pixels = Vec::with_capacity(sizes().map(blocks).sum());
    for (i, (level, size)) in reader.levels().zip(sizes()).enumerate() {
        let len = blocks(size);
        if zstd {
            let at = pixels.len();
            pixels.resize(at + len, 0);
            let written = zstd::bulk::decompress_to_buffer(level.data, &mut pixels[at..])
                .map_err(|error| format!("its level {i} doesn't decompress: {error}"))?;
            if written != len {
                return Err(format!(
                    "its level {i} decompresses to {written} bytes, not {len}"
                ));
            }
        } else {
            pixels.extend_from_slice(level.data);
        }
    }
    Ok(Image {
        size,
        pixels,
        format: Format::Bc {
            compressed,
            mipmapped,
        },
    })
}

/// Where a BC7 DDS file's blocks start: past the magic `DDS `, the 124-byte header and the 20-byte
/// DX10 header naming the DXGI format, which BC7 needs.
const DDS_BLOCKS: usize = 148;

pub(crate) fn is_dds(bytes: &[u8]) -> bool {
    bytes.starts_with(b"DDS ")
}

/// Reads a DDS file, as a model's `MSFT_texture_dds` image is, as far as its headers, keeping
/// `bytes` for [`Image::decoded`]. It's a 2D BC7 image, sRGB or values per its DXGI format, the
/// first level alone or the full chain [`mip_sizes`] gives, blocks filling the file to its end as
/// Microsoft's "Programming Guide for DDS" lays them out. The error says what's wrong with it.
///
/// Fails before allocating for the pixels if a side is 0 or over [`MAX_TEXTURE_SIZE`], or the file
/// is over [`MAX_FILE`] bytes.
pub(crate) fn read_dds(bytes: Vec<u8>) -> Result<Image, String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(format!("it's more than {} MiB", MAX_FILE >> 20));
    }
    let (size, compressed, mipmapped) = dds_header(&bytes)?;
    Ok(Image {
        size,
        pixels: bytes,
        format: Format::Dds {
            compressed,
            mipmapped,
        },
    })
}

/// A DDS file's size, compression and full-chain flag, from its headers; checks the blocks fill it.
fn dds_header(bytes: &[u8]) -> Result<([u32; 2], Compressed, bool), String> {
    if !is_dds(bytes) || bytes.len() < DDS_BLOCKS {
        return Err(format!(
            "it doesn't start with `DDS `, or ends within the {DDS_BLOCKS} bytes of its headers"
        ));
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    // The header's size and the pixel format's, fixed by the format.
    if word(4) != 124 || word(76) != 32 {
        return Err("its header isn't a DDS header".to_owned());
    }
    // DDPF_FOURCC, and the four characters that say a DX10 header follows.
    if word(80) & 0x4 == 0 || &bytes[84..88] != b"DX10" {
        return Err("it has no DX10 header, which names BC7".to_owned());
    }
    let compressed = match word(128) {
        98 => Compressed::Bc7Unorm,
        99 => Compressed::Bc7Srgb,
        format => {
            return Err(format!(
                "it holds DXGI format {format}, where the engine reads BC7 alone, 98 and 99"
            ));
        }
    };
    // A 2D texture, not a cube, and only one.
    if word(132) != 3 || word(136) & 0x4 != 0 || word(140) != 1 {
        return Err("it isn't one 2D image".to_owned());
    }
    let size = [word(16), word(12)];
    check_size(size[0], size[1])?;
    // DDSD_MIPMAPCOUNT: without it, or with a count of 0, it has one level.
    let levels = if word(8) & 0x2_0000 != 0 {
        word(28).max(1)
    } else {
        1
    };
    let chain = mip_sizes(size[0], size[1]).count() as u32;
    let mipmapped = match levels {
        1 => false,
        count if count == chain => true,
        count => {
            return Err(format!(
                "it has {count} levels, neither 1 nor the {chain} of its full chain"
            ));
        }
    };
    let sizes = mip_sizes(size[0], size[1]).take(levels as usize);
    let len: usize = sizes.map(blocks).sum();
    let held = bytes.len() - DDS_BLOCKS;
    if held != len {
        return Err(format!(
            "it holds {held} bytes past its headers, not the {len} of its levels' blocks"
        ));
    }
    Ok((size, compressed, mipmapped))
}

fn decode_dds(bytes: &[u8]) -> Result<Image, String> {
    let (size, compressed, mipmapped) = dds_header(bytes)?;
    Ok(Image {
        size,
        pixels: bytes[DDS_BLOCKS..].to_vec(),
        format: Format::Bc {
            compressed,
            mipmapped,
        },
    })
}

/// Reads a PNG or JPEG as far as its header, keeping `bytes` for [`Image::decoded`] to decode to
/// RGBA8. The first bytes tell them apart, whatever the file's name or a model says. Color
/// profiles and gamma are ignored, as glTF requires: colors are sRGB-encoded. The error says
/// what's wrong with the image.
///
/// Fails before decoding if a side is 0 or over [`MAX_TEXTURE_SIZE`], the file is over
/// [`MAX_FILE`] bytes, or a JPEG ends before its end-of-image marker. What's past the header, such
/// as a PNG's compressed rows, fails only as it decodes.
pub(crate) fn read(bytes: Vec<u8>) -> Result<Image, String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(format!("it's more than {} MiB", MAX_FILE >> 20));
    }
    let read = || {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            read_png(&bytes).map(|size| (size, Format::Png))
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            read_jpeg(&bytes).map(|size| (size, Format::Jpeg))
        } else {
            Err("it starts with neither a PNG's signature nor a JPEG's".to_owned())
        }
    };
    // zune-jpeg can panic on a malformed file (its README allows it), png only by a bug; either
    // way the file's doing, so it fails like an undecodable image. gg-core's panic hook logs it.
    let (size, format) = panic::catch_unwind(AssertUnwindSafe(read))
        .unwrap_or_else(|_| Err("its decoder panicked on it".to_owned()))?;
    Ok(Image {
        size,
        pixels: bytes,
        format,
    })
}

/// Whether `bytes` start with a Radiance HDR signature: `#?RADIANCE`, or the older `#?RGBE`.
fn is_hdr(bytes: &[u8]) -> bool {
    bytes.starts_with(b"#?RADIANCE") || bytes.starts_with(b"#?RGBE")
}

/// A Radiance HDR decoder, read to the header: strict, except for the older signature, which only
/// a decoder that takes the signature as a comment reads.
fn hdr_decoder(bytes: &[u8]) -> Result<HdrDecoder<Cursor<&[u8]>>, String> {
    let strict = bytes.starts_with(b"#?RADIANCE");
    HdrDecoder::with_strictness(Cursor::new(bytes), strict).map_err(|error| error.to_string())
}

/// Reads a Radiance HDR as far as its header, keeping `bytes` for [`Image::decoded`] to decode to
/// 16-bit-float RGBA. The error says what's wrong with it.
///
/// Fails before decoding if a side is 0 or over [`MAX_TEXTURE_SIZE`], or the file is over
/// [`MAX_FILE`] bytes. What's past the header fails only as it decodes.
pub(crate) fn read_hdr(bytes: Vec<u8>) -> Result<Image, String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(format!("it's more than {} MiB", MAX_FILE >> 20));
    }
    // Panics only by a bug; still the file's doing, as for a PNG.
    let size = panic::catch_unwind(|| {
        let (width, height) = hdr_decoder(&bytes)?.dimensions();
        check_size(width, height).map(|()| [width, height])
    })
    .unwrap_or_else(|_| Err("its decoder panicked on it".to_owned()))?;
    Ok(Image {
        size,
        pixels: bytes,
        format: Format::Hdr,
    })
}

/// Decodes a [`read_hdr`] Radiance HDR to linear 16-bit-float RGBA, alpha 1: each RGBE texel (run-
/// length encoded or flat) to its color, clamped to 65,504, the most a 16-bit float holds short of
/// infinity (a sky's sun disk can exceed it).
fn decode_hdr(bytes: &[u8]) -> Result<Image, String> {
    let decoder = hdr_decoder(bytes)?;
    let (width, height) = decoder.dimensions();
    check_size(width, height)?;
    // Three 4-byte floats a texel become four 2-byte ones in place, front to back; each texel is
    // read before it's written over.
    let count = width as usize * height as usize;
    let mut pixels = vec![0; count * 12];
    (decoder.read_image(&mut pixels)).map_err(|error| error.to_string())?;
    for texel in 0..count {
        let from = texel * 12;
        let channel = |i: usize| {
            let bytes = [0, 1, 2, 3].map(|byte| pixels[from + 4 * i + byte]);
            f32::from_ne_bytes(bytes)
        };
        let rgba = [channel(0), channel(1), channel(2), 1.0];
        for (i, value) in rgba.into_iter().enumerate() {
            let half = f16::from_f32(value.min(f16::MAX.to_f32()));
            let to = texel * 8 + 2 * i;
            pixels[to..to + 2].copy_from_slice(&half.to_ne_bytes());
        }
    }
    pixels.truncate(count * 8);
    Ok(Image {
        size: [width, height],
        pixels,
        format: Format::Rgba16f,
    })
}

/// Fails unless each side is 1 to [`MAX_TEXTURE_SIZE`] pixels.
fn check_size(width: u32, height: u32) -> Result<(), String> {
    let sides = 1..=MAX_TEXTURE_SIZE;
    if sides.contains(&width) && sides.contains(&height) {
        Ok(())
    } else {
        Err(format!(
            "it's {width}x{height} pixels, where each side is 1 to {MAX_TEXTURE_SIZE}"
        ))
    }
}

/// A PNG decoder that expands any file to RGBA or gray with alpha, 8 bits a sample.
fn png_decoder(bytes: &[u8]) -> png::Decoder<Cursor<&[u8]>> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::ALPHA | png::Transformations::STRIP_16);
    decoder
}

/// Reads a PNG's chunks up to the first compressed rows, which hold the pixels: its size.
fn read_png(bytes: &[u8]) -> Result<[u32; 2], String> {
    let png = |error: png::DecodingError| error.to_string();
    let mut decoder = png_decoder(bytes);
    let header = decoder.read_header_info().map_err(png)?;
    let size = [header.width, header.height];
    check_size(size[0], size[1])?;
    decoder.read_info().map_err(png)?;
    Ok(size)
}

/// Decodes a PNG of any color type and bit depth to RGBA8: gray to equal red, green and blue,
/// palettes to colors, 16-bit samples to their high bytes, missing alpha to opaque.
pub(crate) fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let png = |error: png::DecodingError| error.to_string();
    let mut decoder = png_decoder(bytes);
    let header = decoder.read_header_info().map_err(png)?;
    check_size(header.width, header.height)?;
    let mut reader = decoder.read_info().map_err(png)?;
    let size =
        (reader.output_buffer_size()).ok_or_else(|| png(png::DecodingError::LimitsExceeded))?;
    let mut decoded = vec![0; size];
    let info = reader.next_frame(&mut decoded).map_err(png)?;
    decoded.truncate(info.buffer_size());
    let pixels = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => decoded,
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => decoded
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[gray, alpha]| [gray, gray, gray, alpha])
            .collect(),
        other => {
            unreachable!("png expands every image to 8-bit RGBA or gray with alpha: {other:?}")
        }
    };
    Ok(Image {
        size: [info.width, info.height],
        pixels,
        format: Format::Rgba8,
    })
}

/// A JPEG decoder to `colorspace`. It refuses a JPEG larger than a texture at the header, before
/// allocating, and fails on a cut or corrupt scan rather than ending it in gray.
fn jpeg_decoder(bytes: &[u8], colorspace: ColorSpace) -> JpegDecoder<ZCursor<&[u8]>> {
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(colorspace)
        .set_max_width(MAX_TEXTURE_SIZE as usize)
        .set_max_height(MAX_TEXTURE_SIZE as usize)
        .set_strict_mode(true);
    JpegDecoder::new_with_options(ZCursor::new(bytes), options)
}

/// Reads a JPEG's headers and walks it to its end: its size.
fn read_jpeg(bytes: &[u8]) -> Result<[u32; 2], String> {
    let mut jpeg = jpeg_decoder(bytes, ColorSpace::RGBA);
    jpeg.decode_headers()
        .map_err(|error: DecodeErrors| error.to_string())?;
    // Strict mode can miss a sequential scan cut in its last MCU row: the zeros zune-jpeg reads
    // past the end finish its blocks. A file ending in an end-of-image marker wasn't cut in a scan
    // (scan data holds none); any other is walked to its marker, if it has one.
    if !bytes.ends_with(&[0xff, 0xd9]) && !reaches_its_end(bytes) {
        return Err("it ends before its end-of-image marker".to_owned());
    }
    let Some(info) = jpeg.info() else {
        unreachable!("a JPEG whose headers read has its size");
    };
    Ok([info.width.into(), info.height.into()])
}

/// Decodes a [`read_jpeg`] JPEG (Huffman-coded, 8-bit, sequential or progressive) to RGBA8: gray
/// to equal red, green and blue; YCbCr, CMYK and YCCK converted, CMYK as Adobe stores it,
/// inverted; RGB as is; alpha opaque.
fn decode_jpeg(bytes: &[u8]) -> Result<Image, String> {
    let jpeg_error = |error: DecodeErrors| error.to_string();
    let mut jpeg = jpeg_decoder(bytes, ColorSpace::RGBA);
    jpeg.decode_headers().map_err(jpeg_error)?;
    // zune-jpeg (0.5.15) makes RGB rightly from every color space, but RGBA only from gray and
    // YCbCr (it scrambles CMYK and YCCK, refuses RGB), and picks its conversion when it reads the
    // headers. So the rest decode afresh to RGB, given alpha below. Its 0.5.16 release candidates
    // fix RGBA for CMYK and YCCK, not RGB.
    let rgb = !matches!(
        jpeg.input_colorspace(),
        Some(ColorSpace::Luma | ColorSpace::YCbCr)
    );
    if rgb {
        jpeg = jpeg_decoder(bytes, ColorSpace::RGB);
    }
    let decoded = jpeg.decode().map_err(jpeg_error)?;
    let (Some(info), Some(colorspace)) = (jpeg.info(), jpeg.input_colorspace()) else {
        unreachable!("a JPEG that decodes has had its header read");
    };
    // zune-jpeg settles some color spaces only while decoding, such as RGB from an Adobe marker's
    // three-component CMYK. Strict mode refuses fewer components than the settled space takes,
    // but takes too few for YCCK as YCbCr, rightly for three. Fewer than that, or more than the
    // space takes (four that a later Adobe marker calls YCbCr), decode to wrong pixels.
    if colorspace.num_components() != usize::from(info.components) {
        return Err(format!(
            "its {} components don't make the gray, YCbCr, RGB, CMYK or YCCK that its markers say \
             it holds",
            info.components
        ));
    }
    let pixels = if rgb {
        (decoded.as_chunks::<3>().0.iter())
            .flat_map(|&[red, green, blue]| [red, green, blue, 255])
            .collect()
    } else {
        decoded
    };
    Ok(Image {
        size: [info.width.into(), info.height.into()],
        pixels,
        format: Format::Rgba8,
    })
}

/// Whether a JPEG's segments (each as long as it says) and scan data (no markers but restarts)
/// run from the start to the end-of-image marker, which a cut file has lost. What follows that
/// marker, such as a motion photo's video, isn't read.
fn reaches_its_end(bytes: &[u8]) -> bool {
    // A marker, not a scan's 0xff, which has a 0 stuffed after it, a restart or a fill byte.
    let marker = |pair: &[u8]| pair[0] == 0xff && !matches!(pair[1], 0 | 0xd0..=0xd7 | 0xff);
    // Past the start-of-image marker.
    let mut at = 2;
    while let Some(found) = bytes
        .get(at..)
        .and_then(|rest| rest.windows(2).position(marker))
    {
        at += found + 2;
        if bytes[at - 1] == 0xd9 {
            return true;
        }
        // Past the segment, whose length counts its own two bytes.
        let Some(&[high, low]) = bytes.get(at..at + 2) else {
            return false;
        };
        at += usize::from(u16::from_be_bytes([high, low]));
    }
    false
}

#[cfg(test)]
pub(crate) mod tests {
    use gg_renderer::Mipmaps;

    use super::*;

    /// A PNG or JPEG read and decoded, as a texture's image is by the time it's drawn.
    fn decode(bytes: &[u8]) -> Result<Image, String> {
        read(bytes.to_vec())?.decoded()
    }

    /// A one-row PNG, with the given palette and transparency chunks.
    pub(crate) fn png(
        color: png::ColorType,
        depth: png::BitDepth,
        width: u32,
        pixels: &[u8],
        palette_and_alpha: Option<(&[u8], &[u8])>,
    ) -> Vec<u8> {
        let mut file = Vec::new();
        let mut encoder = png::Encoder::new(&mut file, width, 1);
        encoder.set_color(color);
        encoder.set_depth(depth);
        if let Some((palette, alpha)) = palette_and_alpha {
            encoder.set_palette(palette);
            encoder.set_trns(alpha);
        }
        let mut writer = encoder.write_header().expect("a header");
        writer.write_image_data(pixels).expect("the pixels");
        writer.finish().expect("a whole PNG");
        file
    }

    /// A KTX2 file laid out as ctt lays out the cooker's.
    pub(crate) fn ktx2(
        format: ktx2::Format,
        size: [u32; 2],
        levels: &[Vec<u8>],
        zstd: bool,
    ) -> Vec<u8> {
        let (dfd, type_size) = ktx2::dfd::Basic::from_format_with(format, false, None, None, None)
            .expect("a data format descriptor");
        let dfd = ktx2::dfd::Block::Basic(dfd).to_vec();
        let stored: Vec<Vec<u8>> = (levels.iter())
            .map(|level| match zstd {
                true => zstd::bulk::compress(level, 3).expect("compressed"),
                false => level.clone(),
            })
            .collect();
        let dfd_at = ktx2::Header::LENGTH + levels.len() * ktx2::LevelIndex::LENGTH;
        let header = ktx2::Header {
            format: Some(format),
            type_size,
            pixel_width: size[0],
            pixel_height: size[1],
            pixel_depth: 0,
            layer_count: 0,
            face_count: 1,
            level_count: levels.len() as u32,
            supercompression_scheme: zstd.then_some(ktx2::SupercompressionScheme::Zstandard),
            index: ktx2::Index {
                dfd_byte_offset: dfd_at as u32,
                dfd_byte_length: 4 + dfd.len() as u32,
                kvd_byte_offset: 0,
                kvd_byte_length: 0,
                sgd_byte_offset: 0,
                sgd_byte_length: 0,
            },
        };
        let mut file = header.as_bytes().to_vec();
        let mut at = (dfd_at + 4 + dfd.len()) as u64;
        for (level, stored) in levels.iter().zip(&stored) {
            let index = ktx2::LevelIndex {
                byte_offset: at,
                byte_length: stored.len() as u64,
                uncompressed_byte_length: level.len() as u64,
            };
            file.extend_from_slice(&index.as_bytes());
            at += stored.len() as u64;
        }
        file.extend_from_slice(&(4 + dfd.len() as u32).to_le_bytes());
        file.extend_from_slice(&dfd);
        stored
            .iter()
            .for_each(|stored| file.extend_from_slice(stored));
        file
    }

    /// A cooked texture reads as its levels' blocks: sRGB colors as BC7; a model's maps as BC7 of
    /// values, a normal map as BC5, taken by a texture of maps as the kind it holds.
    #[test]
    fn a_cooked_texture_reads_as_its_levels_blocks() {
        const BC7_SRGB_BLOCK: ktx2::Format = ktx2::Format::BC7_SRGB_BLOCK;
        // 5x5 texels are 2x2 blocks; the chain's 2x2 and 1x1 levels a block each.
        let blocks = |count: usize, byte: u8| vec![byte; count * 16];
        let one = ktx2(BC7_SRGB_BLOCK, [5, 5], &[blocks(4, 1)], true);
        let image = decode_ktx2(&one).expect("a level");
        let colors = |mipmapped| Format::Bc {
            compressed: Compressed::Bc7Srgb,
            mipmapped,
        };
        assert_eq!((image.size, image.format), ([5, 5], colors(false)));
        assert_eq!(image.pixels, blocks(4, 1));
        let weighted = Texels::Color(Mipmaps::AlphaWeighted);
        assert_eq!(image.texels(weighted), Texels::Bc7 { mipmapped: false });
        let levels = [blocks(4, 1), blocks(1, 2), blocks(1, 3)];
        let chain = ktx2(BC7_SRGB_BLOCK, [5, 5], &levels, false);
        let image = decode_ktx2(&chain).expect("its chain");
        assert_eq!(image.format, colors(true));
        assert_eq!(image.pixels, levels.concat());
        for (format, compressed, map) in [
            (
                ktx2::Format::BC7_UNORM_BLOCK,
                Compressed::Bc7Unorm,
                Map::Surface,
            ),
            (ktx2::Format::BC5_UNORM_BLOCK, Compressed::Bc5, Map::Normal),
            // A normal map as the cooker keeps a DDS image's.
            (
                ktx2::Format::BC7_UNORM_BLOCK,
                Compressed::Bc7Unorm,
                Map::Normal,
            ),
        ] {
            let image = decode_ktx2(&ktx2(format, [5, 5], &levels, true)).expect("a map's chain");
            let format = Format::Bc {
                compressed,
                mipmapped: true,
            };
            assert_eq!(
                (image.format, image.compressed()),
                (format, Some(compressed))
            );
            let cooked = Texels::CompressedMap {
                map,
                bc5: compressed == Compressed::Bc5,
                mipmapped: true,
            };
            assert_eq!(image.texels(Texels::Map(map)), cooked);
            assert!(compressed.holds(Texels::Map(map)), "{compressed:?}");
        }
        assert_eq!(Compressed::of(Texels::Map(Map::Normal)), Compressed::Bc5);
        let colors = Texels::Color(Mipmaps::Averaged);
        assert!(!Compressed::Bc7Unorm.holds(colors) && !Compressed::Bc5.holds(colors));
        assert!(!Compressed::Bc5.holds(Texels::Map(Map::Surface)));
    }

    /// A BC7 DDS file of DXGI `format` (98 values, 99 sRGB), laid out as Microsoft's guide and
    /// etcpak, which made the Bistro's, do.
    pub(crate) fn dds(format: u32, size: [u32; 2], levels: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = b"DDS ".to_vec();
        let mut words = [0u32; 31];
        // The header's size; DDSD_CAPS, HEIGHT, WIDTH, PIXELFORMAT, MIPMAPCOUNT and LINEARSIZE.
        words[0] = 124;
        words[1] = 0xa_1007;
        [words[2], words[3]] = [size[1], size[0]];
        words[4] = levels[0].len() as u32;
        words[6] = levels.len() as u32;
        // The pixel format's size, DDPF_FOURCC, its four characters; DDSCAPS_TEXTURE, MIPMAP and
        // COMPLEX.
        words[18] = 32;
        words[19] = 0x4;
        words[20] = u32::from_le_bytes(*b"DX10");
        words[26] = 0x40_1008;
        bytes.extend(words.iter().flat_map(|word| word.to_le_bytes()));
        // The DX10 header: DXGI format, a 2D texture, not a cube, one of them.
        for word in [format, 3, 0, 1, 0] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(levels.concat());
        bytes
    }

    /// A BC7 DDS reads as far as its headers and decodes to its levels' blocks, as colors or a map
    /// per what samples it, sRGB or values alike.
    #[test]
    fn a_dds_of_bc7_decodes_to_its_levels_blocks() {
        let blocks = |count: usize, byte: u8| vec![byte; count * 16];
        let levels = [blocks(4, 1), blocks(1, 2), blocks(1, 3)];
        for (format, compressed) in [(99, Compressed::Bc7Srgb), (98, Compressed::Bc7Unorm)] {
            let file = dds(format, [5, 5], &levels);
            let read = read_dds(file).expect("a DDS");
            let dds = Format::Dds {
                compressed,
                mipmapped: true,
            };
            assert_eq!((read.size, read.format), ([5, 5], dds));
            assert_eq!(read.decoded_len(), 6 * 16);
            let colors = Texels::Color(Mipmaps::Averaged);
            assert_eq!(read.texels(colors), Texels::Bc7 { mipmapped: true });
            let normals = Texels::CompressedMap {
                map: Map::Normal,
                bc5: false,
                mipmapped: true,
            };
            assert_eq!(read.texels(Texels::Map(Map::Normal)), normals);
            let decoded = read.decoded().expect("its blocks");
            let bc = Format::Bc {
                compressed,
                mipmapped: true,
            };
            assert_eq!((decoded.format, decoded.pixels), (bc, levels.concat()));
        }
        let one = read_dds(dds(99, [5, 5], &[blocks(4, 1)])).expect("a level");
        let one = one.decoded().expect("its blocks");
        assert_eq!(
            (one.format, one.pixels.len()),
            (
                Format::Bc {
                    compressed: Compressed::Bc7Srgb,
                    mipmapped: false,
                },
                4 * 16
            )
        );
    }

    /// A DDS that isn't one 2D BC7 image of one level or the full chain, blocks filling the file,
    /// is refused, saying why.
    #[test]
    fn a_dds_that_isnt_one_of_bc7_is_refused() {
        let blocks = |count: usize| vec![0; count * 16];
        let levels = [blocks(4), blocks(1), blocks(1)];
        let good = dds(98, [5, 5], &levels);
        let with = |at: usize, word: u32| {
            let mut file = good.clone();
            file[at..at + 4].copy_from_slice(&word.to_le_bytes());
            file
        };
        let cases = [
            (good[..100].to_vec(), "within"),
            (with(0, u32::from_le_bytes(*b"DDT ")), "DDS"),
            (with(84, u32::from_le_bytes(*b"DXT5")), "DX10"),
            (with(128, 83), "DXGI format 83"),
            (with(132, 4), "2D"),
            (with(136, 0x4), "2D"),
            (with(140, 6), "2D"),
            (with(16, 0), "0x5"),
            (with(28, 2), "2 levels"),
            (good[..good.len() - 1].to_vec(), "past its headers"),
            ([good.clone(), vec![0]].concat(), "past its headers"),
        ];
        for (file, named) in cases {
            let problem = read_dds(file).expect_err(named);
            assert!(problem.contains(named), "{problem}");
        }
    }

    #[test]
    fn a_cooked_texture_that_isnt_the_cookers_is_refused() {
        const BC7_SRGB_BLOCK: ktx2::Format = ktx2::Format::BC7_SRGB_BLOCK;
        const BC4_UNORM_BLOCK: ktx2::Format = ktx2::Format::BC4_UNORM_BLOCK;
        const R8G8B8A8_SRGB: ktx2::Format = ktx2::Format::R8G8B8A8_SRGB;
        let blocks = |count: usize| vec![0; count * 16];
        let mut short = ktx2(BC7_SRGB_BLOCK, [5, 5], &[blocks(3)], true);
        let mut cut = ktx2(BC7_SRGB_BLOCK, [4, 4], &[blocks(1)], false);
        cut.truncate(cut.len() - 1);
        // A level whose index says it decompresses to its blocks, though it decompresses to fewer.
        let level = ktx2::Header::LENGTH;
        short[level + 16..level + 24].copy_from_slice(&64u64.to_le_bytes());
        // The data format descriptor's first block side, past its length, block header, color
        // model, primaries, transfer and flags, set to 256 texels.
        let mut side = ktx2(BC7_SRGB_BLOCK, [4, 4], &[blocks(1)], false);
        side[level + ktx2::LevelIndex::LENGTH + 16] = 0xff;
        let cases = [
            ("not KTX2", b"\x89PNG\r\n\x1a\n".to_vec()),
            ("RGBA8", ktx2(R8G8B8A8_SRGB, [1, 1], &[vec![0; 4]], false)),
            (
                "BC4, which the cooker doesn't write",
                ktx2(BC4_UNORM_BLOCK, [4, 4], &[blocks(1)], false),
            ),
            (
                "two of three levels",
                ktx2(BC7_SRGB_BLOCK, [5, 5], &[blocks(4), blocks(1)], false),
            ),
            (
                "too few blocks",
                ktx2(BC7_SRGB_BLOCK, [5, 5], &[blocks(3)], false),
            ),
            ("too few blocks decompressed", short),
            ("a block 256 texels across", side.clone()),
            (
                "too large",
                ktx2(BC7_SRGB_BLOCK, [9000, 1], &[blocks(2250)], true),
            ),
            ("cut short", cut),
        ];
        for (case, file) in cases {
            assert!(decode_ktx2(&file).is_err(), "{case}");
        }
        // ktx2 panics here; the panic is caught.
        let refused = decode_ktx2(&side).expect_err("a block 256 texels across");
        assert!(refused.contains("panicked"), "{refused}");
    }

    /// A header reads apart from the pixels, kept as the file's bytes: size, texture contents and
    /// decoded length are known at once; it decodes later, as the loader does. A PNG or KTX2 cut
    /// short or corrupt past the header reads and fails on decode; a cut JPEG fails on read, when
    /// walked to its end.
    #[test]
    fn an_images_header_reads_apart_from_its_pixels() {
        let rgb = &[10, 20, 30, 40, 50, 60];
        let png = png(png::ColorType::Rgb, png::BitDepth::Eight, 2, rgb, None);
        let from_png = read(png.clone()).expect("its header");
        let averaged = Texels::Color(Mipmaps::Averaged);
        assert_eq!((from_png.size, from_png.format), ([2, 1], Format::Png));
        assert_eq!(from_png.texels(averaged), averaged);
        let normals = Texels::Map(Map::Normal);
        assert_eq!(from_png.texels(normals), normals, "as the model samples it");
        assert_eq!((&from_png.pixels, from_png.decoded_len()), (&png, 8));
        let decoded = from_png.decoded().expect("its pixels");
        assert_eq!(decoded.format, Format::Rgba8);
        assert_eq!(decoded.pixels, [10, 20, 30, 255, 40, 50, 60, 255]);

        let jpeg = jpeg();
        let from_jpeg = read(jpeg.clone()).expect("its headers");
        assert_eq!((from_jpeg.size, from_jpeg.format), ([16, 16], Format::Jpeg));
        assert_eq!(from_jpeg.decoded_len(), 16 * 16 * 4);
        let decoded = from_jpeg.decoded().expect("its pixels");
        assert_eq!((decoded.size, decoded.format), ([16, 16], Format::Rgba8));
        let cut = read(jpeg[..jpeg.len() - 20].to_vec());
        assert!(cut.expect_err("cut short").contains("end-of-image"));

        // 5x5 texels are 2x2 blocks; the chain's 2x2 and 1x1 levels a block each.
        const BC7_SRGB_BLOCK: ktx2::Format = ktx2::Format::BC7_SRGB_BLOCK;
        let levels = [vec![1; 64], vec![2; 16], vec![3; 16]];
        let file = ktx2(BC7_SRGB_BLOCK, [5, 5], &levels, true);
        let cooked = read_ktx2(file.clone()).expect("its header");
        let format = Format::Ktx2 {
            compressed: Compressed::Bc7Srgb,
            mipmapped: true,
            zstd: true,
        };
        assert_eq!((cooked.size, cooked.format), ([5, 5], format));
        let chain = Texels::Bc7 { mipmapped: true };
        assert_eq!(cooked.texels(Texels::Color(Mipmaps::None)), chain);
        assert_eq!((&cooked.pixels, cooked.decoded_len()), (&file, 96));
        let decoded = cooked.decoded().expect("its levels");
        let blocks = Format::Bc {
            compressed: Compressed::Bc7Srgb,
            mipmapped: true,
        };
        assert_eq!(decoded.format, blocks);
        assert_eq!(decoded.pixels, levels.concat());

        let cut = read(png[..png.len() - 20].to_vec()).expect("its header, whole");
        assert!(cut.decoded().is_err(), "its rows cut short");
        // A level whose index says it decompresses to its 4 blocks, though it decompresses to 3.
        let mut short = ktx2(BC7_SRGB_BLOCK, [5, 5], &[vec![0; 48]], true);
        let level = ktx2::Header::LENGTH;
        short[level + 16..level + 24].copy_from_slice(&64u64.to_le_bytes());
        let short = read_ktx2(short).expect("its header, whose level says it's whole");
        assert!(short.decoded().is_err(), "its level decompressed short");
    }

    /// A Radiance HDR of either signature reads to its header and decodes to 16-bit-float RGBA,
    /// alpha 1, each texel its RGBE's color clamped to the most a 16-bit float holds. A texture of
    /// it holds high-dynamic-range colors however it's added. Cut short, it fails on decode; too
    /// large, on read.
    #[test]
    fn a_radiance_hdr_decodes_to_16_bit_floats() {
        // 1, 0.5 and 0.25; 200, 100 and 50; and 255 times 2^14, then none.
        let texels: [[u8; 4]; 3] = [[128, 64, 32, 129], [200, 100, 50, 136], [255, 0, 0, 150]];
        let hdr = |signature: &str, width: u32| {
            let header = format!("{signature}\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X {width}\n");
            [header.as_bytes(), texels.as_flattened()].concat()
        };
        for signature in ["#?RADIANCE", "#?RGBE"] {
            let read = read_hdr(hdr(signature, 3)).expect(signature);
            assert_eq!((read.size, read.format), ([3, 1], Format::Hdr));
            assert_eq!(read.texels(Texels::Color(Mipmaps::Averaged)), Texels::Hdr);
            assert_eq!(read.decoded_len(), 3 * 8);
            let decoded = read.decoded().expect(signature);
            assert_eq!((decoded.size, decoded.format), ([3, 1], Format::Rgba16f));
            let values: Vec<f32> = (decoded.pixels.as_chunks::<2>().0.iter())
                .map(|&bytes| f16::from_ne_bytes(bytes).to_f32())
                .collect();
            let expected = [
                1.0, 0.5, 0.25, 1.0, 200.0, 100.0, 50.0, 1.0, 65504.0, 0.0, 0.0, 1.0,
            ];
            assert_eq!(values, expected, "{signature}");
        }
        let mut cut = hdr("#?RADIANCE", 3);
        cut.truncate(cut.len() - 2);
        let cut = read_hdr(cut).expect("its header, whole");
        assert!(cut.decoded().is_err(), "its texels cut short");
        let wide = read_hdr(hdr("#?RADIANCE", 9000)).expect_err("too wide");
        assert!(wide.contains("9000") && wide.contains("8192"), "{wide}");
    }

    #[test]
    fn every_kind_of_png_decodes_to_rgba8() {
        use png::BitDepth::{Eight, One, Sixteen};
        use png::ColorType::{Grayscale, GrayscaleAlpha, Indexed, Rgb, Rgba};
        let cases: [(&str, Vec<u8>, [u8; 8]); 5] = [
            (
                "RGB",
                png(Rgb, Eight, 2, &[10, 20, 30, 40, 50, 60], None),
                [10, 20, 30, 255, 40, 50, 60, 255],
            ),
            (
                "gray with alpha",
                png(GrayscaleAlpha, Eight, 2, &[100, 7, 200, 250], None),
                [100, 100, 100, 7, 200, 200, 200, 250],
            ),
            (
                // The first color transparent; the second, with no alpha entry, opaque.
                "a palette with alpha",
                png(
                    Indexed,
                    Eight,
                    2,
                    &[1, 0],
                    Some((&[1, 2, 3, 4, 5, 6], &[0])),
                ),
                [4, 5, 6, 255, 1, 2, 3, 0],
            ),
            (
                "16-bit RGBA",
                png(
                    Rgba,
                    Sixteen,
                    2,
                    &[
                        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0xff, 0xff, 0, 0, 0x80, 0,
                        0, 0xff,
                    ],
                    None,
                ),
                [0x12, 0x56, 0x9a, 0xde, 0xff, 0, 0x80, 0],
            ),
            (
                "1-bit gray",
                png(Grayscale, One, 2, &[0b1000_0000], None),
                [255, 255, 255, 255, 0, 0, 0, 255],
            ),
        ];
        for (kind, file, rgba) in cases {
            let image = decode(&file).expect("a PNG");
            assert_eq!(image.size, [2, 1], "{kind}");
            assert_eq!(image.pixels, rgba, "{kind}");
        }
    }

    #[test]
    fn a_texture_that_doesnt_load_names_itself_and_its_file() {
        let missing = Path::new("no such folder/checkerboard.png");
        let not_a_png = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        for path in [missing, &not_a_png] {
            let error = load_image("checkerboard", path, &Files::default()).expect_err("no image");
            assert_eq!(error.code(), "engine.bad_texture", "{}", path.display());
            let message = error.to_string();
            assert!(
                message.contains("\"checkerboard\"")
                    && message.contains(&path.display().to_string()),
                "{message}"
            );
        }
        // The cause appears once: as the error's source, not in its message too.
        let error = load_image("checkerboard", missing, &Files::default()).expect_err("no image");
        let shown = format!("{error:#}");
        assert_eq!(shown.matches("(os error").count(), 1, "{shown}");
    }

    /// A 16x16 baseline JPEG from Windows' encoder at quality 100, quadrants of `QUADRANTS`' colors
    /// (top left, top right, bottom left, bottom right).
    fn jpeg() -> Vec<u8> {
        let base64 = concat!(
            "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
            "AQEBAQEBAQEBAQEBAQEBAQEBAQH/2wBDAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
            "AQEBAQEBAQEBAQEBAQEBAQEBAQH/wAARCAAQABADASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcI",
            "CQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRol",
            "JicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ip",
            "qrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAA",
            "AAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLR",
            "ChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaX",
            "mJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEA",
            "PwD8v6+yK/Oev72K/A/p5+EP/Elv/EKv+Mh/4iV/xEr/AF5/5lP+p39i/wCp3+p//Uz4q/tH+0f9av8AqA+p/UP+Yr61",
            "/s/3X0peMf8Aio5/qL/wnf8AEG/+IN/6z/8AMZ/xEP8A1j/4iH/q9/1C8Df2R/ZH+o3/AFNP7Q/tT/mC+pf7X//Z",
        );
        crate::gltf::base64(base64).expect("base64")
    }

    const QUADRANTS: [[u8; 3]; 4] = [[200, 40, 40], [40, 160, 60], [50, 70, 190], [230, 210, 80]];

    /// A 16x16 baseline CMYK JPEG, inverted as Adobe stores it, from libjpeg-turbo via Pillow at
    /// quality 100: `QUADRANTS`' colors as ink, black ink 0, 20, 40 and 60%, making `INKED`.
    fn cmyk() -> Vec<u8> {
        let base64 = concat!(
            "/9j/7gAOQWRvYmUAZAAAAAAA/9sAQwABAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
            "AQEBAQEBAQEBAQEBAQEBAQEB/8AAFAgAEAAQBEMRAE0RAFkRAEsRAP/EABgAAQEBAQEAAAAAAAAAAAAAAAoLCQcI/8QA",
            "FBABAAAAAAAAAAAAAAAAAAAAAP/aAA4EQwBNAFkASwAAPwBIAn4n5/CX+eA9QMZ3UBL1BBjOrQKMAR7Gd//Z",
        );
        crate::gltf::base64(base64).expect("base64")
    }

    /// The image `cmyk` holds, from Windows' encoder at quality 100: YCCK, cyan, magenta and yellow
    /// as subsampled YCbCr beside black.
    fn ycck() -> Vec<u8> {
        let base64 = concat!(
            "/9j/7gAOQWRvYmUAZAAAAAAC/9sAQwABAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
            "AQEBAQEBAQEBAQEBAQEBAQEB/9sAQwEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
            "AQEBAQEBAQEBAQEBAQEBAQEB/8AAFAgAEAAQBAEiAAIRAQMRAQQiAP/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYH",
            "CAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBka",
            "JSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeo",
            "qaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAA",
            "AAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy",
            "0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWW",
            "l5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/aAA4EAQACEQMR",
            "BAAAPwD9OK+N6/Riv4J6/fPoGeL3/E6X/EVf+Me/4hr/AMQ1/wBRv+Zt/rj/AG1/rj/rh/1LOFf7O/s7/VX/AKj/AK59",
            "f/5hfqv+0fC/Ra4O/wCKcf8Ar1/wo/8AEZP+Iyf6sf8AMH/xDz/Vz/iHn+sP/UVxz/a/9r/68/8AUr/s/wDsv/mN+u/7",
            "J/fxX4z1+M9fjPX/2Q==",
        );
        crate::gltf::base64(base64).expect("base64")
    }

    /// `QUADRANTS` darkened by `cmyk`'s black ink.
    const INKED: [[u8; 3]; 4] = [[200, 40, 40], [32, 128, 48], [30, 42, 114], [92, 84, 32]];

    /// The YCbCr a JPEG holds for `color`, by JFIF's formulas.
    fn ycbcr(color: [u8; 3]) -> [u8; 3] {
        let [r, g, b] = color.map(f32::from);
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        let cb = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
        let cr = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
        [y, cb, cr].map(|channel| channel.round() as u8)
    }

    #[test]
    fn a_jpeg_decodes_to_rgba8() {
        // The JPEG; with components named R, G and B (as some encoders name RGB ones), so its YCbCr
        // decodes as RGB; with an Adobe marker of transform 0, which says the same; and followed
        // by what looks like a scan header, as a motion photo's video can.
        let jpeg = jpeg();
        let followed = [&jpeg[..], b"\xff\xda"].concat();
        let mut rgb = jpeg.clone();
        let frame = (rgb.windows(2).position(|marker| marker == [0xff, 0xc0]))
            .expect("a baseline JPEG's frame header");
        let scan =
            (rgb.windows(2).position(|marker| marker == [0xff, 0xda])).expect("a scan header");
        for (i, &name) in b"RGB".iter().enumerate() {
            rgb[frame + 10 + 3 * i] = name;
            rgb[scan + 5 + 2 * i] = name;
        }
        let adobe = b"\xff\xee\x00\x0eAdobe\x00\x64\x00\x00\x00\x00\x00";
        let adobe = [&jpeg[..2], adobe, &jpeg[2..]].concat();
        for (kind, file, colors) in [
            ("YCbCr", jpeg, QUADRANTS),
            ("YCbCr followed by more", followed, QUADRANTS),
            ("RGB", rgb, QUADRANTS.map(ycbcr)),
            ("RGB by its Adobe marker", adobe, QUADRANTS.map(ycbcr)),
            ("CMYK", cmyk(), INKED),
            ("YCCK", ycck(), INKED),
        ] {
            let image = decode(&file).expect(kind);
            assert_eq!(image.size, [16, 16], "{kind}");
            // JPEG loses a little: each quadrant, but for blurred edge pixels, is within 2 of its
            // color, and opaque.
            for (quadrant, color) in colors.iter().enumerate() {
                let corner = [quadrant % 2 * 8, quadrant / 2 * 8];
                for (x, y) in (2..6).flat_map(|x| (2..6).map(move |y| (x, y))) {
                    let at = ((corner[1] + y) * 16 + corner[0] + x) * 4;
                    let pixel = &image.pixels[at..at + 4];
                    assert!(
                        pixel[3] == 255 && (0..3).all(|c| pixel[c].abs_diff(color[c]) <= 2),
                        "{kind}, quadrant {quadrant} at ({x}, {y}): {pixel:?}, not {color:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_jpeg_whose_components_make_no_color_space_is_refused() {
        // `cmyk` cut to two components, too few for CMYK, which zune-jpeg refuses; and `cmyk` with
        // its Adobe marker moved after the frame header, saying YCbCr, which zune-jpeg then decodes
        // its four components as.
        let cmyk = cmyk();
        let mut two = cmyk.clone();
        let frame = (two.windows(2).position(|marker| marker == [0xff, 0xc0]))
            .expect("a baseline JPEG's frame header");
        (two[frame + 3], two[frame + 9]) = (8 + 2 * 3, 2);
        two.drain(frame + 16..frame + 22);
        let scan =
            (two.windows(2).position(|marker| marker == [0xff, 0xda])).expect("a scan header");
        (two[scan + 3], two[scan + 4]) = (6 + 2 * 2, 2);
        two.drain(scan + 9..scan + 13);
        // The Adobe marker is first; its last byte, the transform, names the components. The frame
        // header ends where the Huffman tables start.
        let tables =
            (cmyk.windows(2).position(|marker| marker == [0xff, 0xc4])).expect("Huffman tables");
        let adobe = [&cmyk[2..17], &[1]].concat();
        let ycbcr = [&cmyk[..2], &cmyk[18..tables], &adobe, &cmyk[tables..]].concat();
        for (kind, file, problem) in [
            ("two components", two, "components but found 2"),
            ("YCbCr of four", ycbcr, "its 4 components"),
        ] {
            let refused = decode(&file).expect_err(kind);
            assert!(refused.contains(problem), "{kind}: {refused}");
        }
    }

    #[test]
    fn an_image_too_large_for_a_texture_is_refused_from_its_header() {
        // A PNG 9000 across, which would decode; and the JPEG with its header changed to say so,
        // which zune-jpeg refuses on reading.
        use png::{BitDepth, ColorType};
        let png = png(ColorType::Grayscale, BitDepth::One, 9000, &[0; 1125], None);
        let mut jpeg = jpeg();
        let frame = (jpeg.windows(2).position(|marker| marker == [0xff, 0xc0]))
            .expect("a baseline JPEG's frame header");
        jpeg[frame + 7..frame + 9].copy_from_slice(&9000u16.to_be_bytes());
        for (kind, file) in [("PNG", png), ("JPEG", jpeg)] {
            let problem = decode(&file).expect_err("too large");
            assert!(
                problem.contains("9000") && problem.contains("8192"),
                "{kind}: {problem}"
            );
        }
    }

    #[test]
    fn bytes_that_arent_a_whole_png_or_jpeg_are_refused() {
        let (jpeg, cmyk) = (jpeg(), cmyk());
        let png = png(
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            1,
            &[1, 2, 3],
            None,
        );
        let cases: [(&str, &[u8]); 8] = [
            ("nothing", &[]),
            ("a GIF", b"GIF89a\x01\x00\x01\x00"),
            ("a PNG's signature cut short", &png[..4]),
            ("a PNG cut short", &png[..png.len() - 20]),
            ("a JPEG's signature cut short", &jpeg[..2]),
            ("a JPEG cut short", &jpeg[..300]),
            // `cmyk` cut in the first of two MCU rows, which zune-jpeg sees when the second starts;
            // and `jpeg` in its only row, which only the lost end shows.
            ("a JPEG's scan cut short", &cmyk[..cmyk.len() - 20]),
            ("a JPEG's last row cut short", &jpeg[..jpeg.len() - 20]),
        ];
        for (kind, bytes) in cases {
            assert!(decode(bytes).is_err(), "{kind}");
        }
    }
}
