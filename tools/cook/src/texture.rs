//! Textures compressed in KTX2, as a game's package holds them. Colors and values go to BC7,
//! normal maps' X and Y to BC5, and colors of high dynamic range, as a sky's, to BC6H.
//!
//! A layer's texture and a sky have one level. A model's images have full mip chains, which the
//! engine makes for the cooker as a run makes them. A model's BC7 DDS images keep their blocks and
//! levels.

use ctt::encoders::Encoder;
use ctt::encoders::bc7enc::Bc7encSettings;
use ctt::encoders::ispc::IspcSettings;
use ctt::{
    AlphaMode, ColorSpace, Container, ConvertSettings, Format, FormatDesc, ImageRef,
    PipelineOutput, Quality, SurfaceRef, TargetFormat, TextureKind,
};
use gg_engine::cook::{Compressed, mip_sizes};
use gg_engine::{Error, Result};

/// The zstd level of each mip level's supercompression: zstd's highest short of its ultra levels,
/// which take far more memory. Blocks shrink little at any level, but a higher one reads back no
/// slower.
const ZSTD_LEVEL: i32 = 19;

/// Texture `name`'s `pixels`, compressed as `compressed` says into KTX2, as
/// [`decode_ktx2`](gg_engine::cook::decode_ktx2) reads it, each level supercompressed by zstd.
///
/// `pixels` are rows of `size` texels, RGBA8 with alpha not multiplied in, or for BC6H RGBA of
/// 16-bit floats. If `mipmapped`, the rest of their full chain of levels follows, smallest last.
///
/// - sRGB colors go to BC7 by bc7enc_rdo's perceptual encoder, and values, as stored, by the same
///   encoder weighing each channel alike.
/// - A normal map's X and Y, in red and green, go to BC5 by Intel's ISPC Texture Compressor, which
///   drops blue and alpha. Colors of high dynamic range go to BC6H by the same, which drops alpha.
///
/// The same pixels compress to the same bytes every time on one machine.
///
/// Fails with `cook.uncompressed` if ctt can't compress them.
///
/// # Panics
///
/// - `pixels` don't fill the levels exactly.
pub(crate) fn compress(
    name: &str,
    size: [u32; 2],
    pixels: &[u8],
    mipmapped: bool,
    compressed: Compressed,
) -> Result<Vec<u8>> {
    let levels = mip_sizes(size[0], size[1]).take(if mipmapped { usize::MAX } else { 1 });
    let texel = if compressed == Compressed::Bc6h { 8 } else { 4 };
    let mut surfaces = Vec::new();
    let mut at = 0;
    for [width, height] in levels {
        let len = width as usize * height as usize * texel as usize;
        surfaces.push(SurfaceRef {
            data: &pixels[at..at + len],
            width,
            height,
            depth: 1,
            stride: width * texel,
            slice_stride: 0,
        });
        at += len;
    }
    assert_eq!(
        at,
        pixels.len(),
        "the pixels fill texture {name:?}'s levels"
    );
    // bc7enc_rdo weighs colors' channels by how much each shows, which a map's values aren't.
    let bc7 = |perceptual| {
        Encoder::Bc7enc(Bc7encSettings {
            perceptual,
            ..Bc7encSettings::default()
        })
    };
    let (input, color_space, output, encoder) = match compressed {
        Compressed::Bc7Srgb => (
            Format::R8G8B8A8_SRGB,
            ColorSpace::Srgb,
            Format::BC7_SRGB_BLOCK,
            bc7(true),
        ),
        Compressed::Bc7Unorm => (
            Format::R8G8B8A8_UNORM,
            ColorSpace::Linear,
            Format::BC7_UNORM_BLOCK,
            bc7(false),
        ),
        Compressed::Bc5 => (
            Format::R8G8B8A8_UNORM,
            ColorSpace::Linear,
            Format::BC5_UNORM_BLOCK,
            Encoder::Intel(IspcSettings::default()),
        ),
        Compressed::Bc6h => (
            Format::R16G16B16A16_SFLOAT,
            ColorSpace::Linear,
            Format::BC6H_UFLOAT_BLOCK,
            Encoder::Intel(IspcSettings::default()),
        ),
    };
    let image = ImageRef {
        surfaces: vec![surfaces],
        kind: TextureKind::Texture2D,
        desc: FormatDesc {
            format: input,
            color_space,
            // Though alpha isn't multiplied in: ctt multiplies it in as it reads and divides it out
            // for the encoder, blackening a transparent texel's color. A run's filtering mixes that
            // into its neighbors, where a development run's texture keeps the color. Taken as
            // opaque, texels reach the encoder as they are, alpha compressed as the colors are.
            alpha: AlphaMode::Opaque,
        },
    };
    let settings = ConvertSettings {
        format: Some(TargetFormat::Compressed {
            format: output,
            encoder,
        }),
        container: Container::ktx2_zstd(ZSTD_LEVEL),
        quality: Quality::Basic,
        // Its levels are the engine's, as given.
        mipmap: false,
        ..ConvertSettings::default()
    };
    let uncompressed = |problem: String| {
        let message = format!("texture {name:?} doesn't compress to {compressed:?}: {problem}");
        Error::new("cook.uncompressed", message)
    };
    match ctt::convert(image, settings) {
        Ok(PipelineOutput::Encoded(bytes)) => Ok(bytes),
        Ok(PipelineOutput::Raw(_)) => Err(uncompressed("ctt gave no KTX2".to_owned())),
        Err(error) => Err(uncompressed(error.to_string())),
    }
}

/// Texture `name`'s BC7 `blocks`, as a model's DDS image holds them, kept as they are in KTX2 as
/// [`compress`] writes it. They're `size` texels' first level, or if `mipmapped` its full chain, of
/// sRGB or values as `compressed` says.
///
/// Fails with `cook.uncompressed` if ctt can't write them.
///
/// # Panics
///
/// - `compressed` isn't BC7.
/// - `blocks` don't fill the levels exactly.
pub(crate) fn keep(
    name: &str,
    size: [u32; 2],
    blocks: &[u8],
    mipmapped: bool,
    compressed: Compressed,
) -> Result<Vec<u8>> {
    let (format, color_space) = match compressed {
        Compressed::Bc7Srgb => (Format::BC7_SRGB_BLOCK, ColorSpace::Srgb),
        Compressed::Bc7Unorm => (Format::BC7_UNORM_BLOCK, ColorSpace::Linear),
        other => panic!("texture {name:?}'s blocks are kept as BC7, not as {other:?}"),
    };
    let levels = mip_sizes(size[0], size[1]).take(if mipmapped { usize::MAX } else { 1 });
    let mut surfaces = Vec::new();
    let mut at = 0;
    for [width, height] in levels {
        // A row of blocks of 4 by 4 texels, 16 bytes each, which cover the level's edges.
        let stride = width.div_ceil(4) * 16;
        let len = (stride * height.div_ceil(4)) as usize;
        surfaces.push(SurfaceRef {
            data: &blocks[at..at + len],
            width,
            height,
            depth: 1,
            stride,
            slice_stride: 0,
        });
        at += len;
    }
    assert_eq!(
        at,
        blocks.len(),
        "the blocks fill texture {name:?}'s levels"
    );
    let image = ImageRef {
        surfaces: vec![surfaces],
        kind: TextureKind::Texture2D,
        desc: FormatDesc {
            format,
            color_space,
            alpha: AlphaMode::Opaque,
        },
    };
    // No format to convert to: ctt passes the blocks through as they are.
    let settings = ConvertSettings {
        container: Container::ktx2_zstd(ZSTD_LEVEL),
        mipmap: false,
        ..ConvertSettings::default()
    };
    let unwritten = |problem: String| {
        let message = format!("texture {name:?}'s blocks don't go into KTX2: {problem}");
        Error::new("cook.uncompressed", message)
    };
    match ctt::convert(image, settings) {
        Ok(PipelineOutput::Encoded(bytes)) => Ok(bytes),
        Ok(PipelineOutput::Raw(_)) => Err(unwritten("ctt gave no KTX2".to_owned())),
        Err(error) => Err(unwritten(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use gg_engine::cook::{Format as ImageFormat, decode_ktx2};

    use super::*;

    /// A level's `blocks`, BC7 or BC5 as `compressed` says, decoded as a GPU does: rows of `size`
    /// RGBA8 texels, top first, BC5's with blue 0 and alpha 255.
    fn decoded(blocks: &[u8], [width, height]: [u32; 2], compressed: Compressed) -> Vec<[u8; 4]> {
        let (width, height) = (width as usize, height as usize);
        let across = width.div_ceil(4);
        let mut texels = vec![[0; 4]; width * height];
        for (i, block) in blocks.as_chunks::<16>().0.iter().enumerate() {
            let mut decoded = [[0; 4]; 16];
            if compressed == Compressed::Bc5 {
                let mut rg = [0; 32];
                bcdec_rs::bc5(block, &mut rg, 8, false);
                for (texel, &[r, g]) in decoded.iter_mut().zip(rg.as_chunks::<2>().0) {
                    *texel = [r, g, 0, 255];
                }
            } else {
                bcdec_rs::bc7(block, decoded.as_flattened_mut(), 16);
            }
            let (left, top) = (i % across * 4, i / across * 4);
            for (j, texel) in decoded.iter().enumerate() {
                let (x, y) = (left + j % 4, top + j / 4);
                // A block that covers the level's edge holds texels past it.
                if x < width && y < height {
                    texels[y * width + x] = *texel;
                }
            }
        }
        texels
    }

    /// Whether each channel of `texel` is within compression's loss of `expected`'s.
    fn close(texel: [u8; 4], expected: [u8; 4]) -> bool {
        (texel.iter().zip(expected)).all(|(&got, want)| got.abs_diff(want) <= 3)
    }

    /// A texture compresses to KTX2 a run reads back: BC7 decoding to its colors, a transparent
    /// texel's too, or its values; or BC5 decoding to a normal map's X and Y.
    #[test]
    fn a_texture_compresses_to_what_it_holds() {
        // 8x4 texels: opaque on the left, transparent but colored on the right.
        let (opaque, clear) = ([201, 41, 91, 255], [31, 201, 61, 0]);
        let texel = |i: usize| if i % 8 < 4 { opaque } else { clear };
        let pixels: Vec<u8> = (0..32).flat_map(texel).collect();
        for compressed in [Compressed::Bc7Srgb, Compressed::Bc7Unorm, Compressed::Bc5] {
            let ktx2 = compress("halves", [8, 4], &pixels, false, compressed).expect("compressed");
            let image = decode_ktx2(&ktx2).expect("KTX2 a run reads");
            assert_eq!(image.size, [8, 4]);
            let format = ImageFormat::Bc {
                compressed,
                mipmapped: false,
            };
            assert_eq!(image.format, format);
            for (i, got) in decoded(&image.pixels, [8, 4], compressed)
                .into_iter()
                .enumerate()
            {
                let [r, g, b, a] = texel(i);
                let expected = match compressed {
                    Compressed::Bc5 => [r, g, 0, 255],
                    Compressed::Bc7Srgb | Compressed::Bc7Unorm => [r, g, b, a],
                    Compressed::Bc6h => unreachable!("RGBA8 isn't compressed as BC6H"),
                };
                assert!(
                    close(got, expected),
                    "{compressed:?}'s texel {i}: {got:?}, not {expected:?}"
                );
            }
        }
    }

    /// Colors of high dynamic range compress to BC6H a run reads back, each block decoding to its
    /// color within compression's loss, far past 1 and below it.
    #[test]
    fn colors_of_high_dynamic_range_compress_to_bc6h() {
        // 8x4 texels, a block of each color.
        let (bright, dim) = ([40.0, 2.0, 0.5], [0.25, 0.75, 0.0]);
        let texel = |i: usize| if i % 8 < 4 { bright } else { dim };
        let pixels: Vec<u8> = (0..32)
            .flat_map(|i| {
                let [r, g, b] = texel(i);
                [r, g, b, 1.0].map(|value: f32| half::f16::from_f32(value).to_ne_bytes())
            })
            .flatten()
            .collect();
        let bc6h = Compressed::Bc6h;
        let ktx2 = compress("sky", [8, 4], &pixels, false, bc6h).expect("compressed");
        let image = decode_ktx2(&ktx2).expect("KTX2 a run reads");
        let format = ImageFormat::Bc {
            compressed: bc6h,
            mipmapped: false,
        };
        assert_eq!((image.size, image.format), ([8, 4], format));
        let blocks = image.pixels.as_chunks::<16>().0;
        assert_eq!(blocks.len(), 2);
        for (block, expected) in blocks.iter().zip([bright, dim]) {
            let mut decoded = [0.0; 48];
            bcdec_rs::bc6h_float(block, &mut decoded, 12, false);
            for got in decoded.as_chunks::<3>().0 {
                let near = (got.iter().zip(expected))
                    .all(|(&got, want)| (got - want).abs() <= 0.02 * want + 0.005);
                assert!(near, "{got:?}, not {expected:?}");
            }
        }
    }

    /// A full chain compresses to KTX2 a run reads back, each level's blocks where a run looks,
    /// largest first, and to the same bytes every time.
    #[test]
    fn a_textures_levels_compress_in_their_order() {
        // 13x5 texels, whose levels are 6x2, 3x1 and 1x1, each a color of its own.
        let colors = [
            [201, 41, 91, 255],
            [31, 201, 61, 255],
            [41, 91, 221, 128],
            [233, 211, 33, 7],
        ];
        let sizes: Vec<[u32; 2]> = mip_sizes(13, 5).collect();
        let pixels: Vec<u8> = (sizes.iter().zip(colors))
            .flat_map(|(&[width, height], color)| color.repeat((width * height) as usize))
            .collect();
        let colored = Compressed::Bc7Srgb;
        let chain = compress("levels", [13, 5], &pixels, true, colored).expect("compressed");
        let image = decode_ktx2(&chain).expect("KTX2 a run reads");
        let format = ImageFormat::Bc {
            compressed: colored,
            mipmapped: true,
        };
        assert_eq!(image.format, format);
        let mut at = 0;
        for (level, (&size, color)) in sizes.iter().zip(colors).enumerate() {
            let len = (size[0].div_ceil(4) * size[1].div_ceil(4) * 16) as usize;
            let texels = decoded(&image.pixels[at..at + len], size, colored);
            assert!(
                texels.iter().all(|&got| close(got, color)),
                "level {level}: {texels:?}"
            );
            at += len;
        }
        assert_eq!(at, image.pixels.len(), "every level's blocks, and no more");
        let again = compress("levels", [13, 5], &pixels, true, colored).expect("compressed");
        assert_eq!(again, chain, "the same bytes");
    }
}
