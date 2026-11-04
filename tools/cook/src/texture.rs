//! Textures compressed in KTX2, as a game's package holds them, by what they hold: colors as BC7,
//! a layer's texture of one level and a model's base colors and emission with their full chain of
//! mip levels; and a model's maps, values as BC7 and normal maps' X and Y as BC5, with theirs,
//! which the engine makes for the cooker as a frame makes a texture's.

use ctt::encoders::Encoder;
use ctt::encoders::bc7enc::Bc7encSettings;
use ctt::encoders::ispc::IspcSettings;
use ctt::{
    AlphaMode, ColorSpace, Container, ConvertSettings, Format, FormatDesc, ImageRef,
    PipelineOutput, Quality, SurfaceRef, TargetFormat, TextureKind,
};
use gg_engine::cook::{Compressed, mip_sizes};
use gg_engine::{Error, Result};

/// The zstd level each of a texture's levels is supercompressed at: zstd's highest short of its
/// ultra levels, which take far more memory to compress. Blocks shrink little whatever the level,
/// but reading them back takes no longer for a higher one.
const ZSTD_LEVEL: i32 = 19;

/// Texture `name`'s `pixels`, rows of RGBA8, alpha not multiplied in, `size` texels across and
/// down, followed, if `mipmapped`, by the rest of their full chain of levels, smallest last:
/// compressed as `compressed` says, each level supercompressed by zstd, in KTX2, as
/// [`decode_ktx2`](gg_engine::cook::decode_ktx2) reads it. Colors, sRGB-encoded, go to BC7 by
/// bc7enc_rdo's perceptual encoder, and values, as stored, to BC7 by the same encoder weighing each
/// channel alike; a normal map's X and Y, in red and green, go to BC5 by Intel's ISPC Texture
/// Compressor, which drops blue and alpha. The same pixels compress to the same bytes every time on
/// one machine.
///
/// Fails with `cook.uncompressed` if ctt can't compress them.
///
/// # Panics
///
/// If `pixels` don't fill the levels exactly.
pub(crate) fn compress(
    name: &str,
    size: [u32; 2],
    pixels: &[u8],
    mipmapped: bool,
    compressed: Compressed,
) -> Result<Vec<u8>> {
    let levels = mip_sizes(size[0], size[1]).take(if mipmapped { usize::MAX } else { 1 });
    let mut surfaces = Vec::new();
    let mut at = 0;
    for [width, height] in levels {
        let len = width as usize * height as usize * 4;
        surfaces.push(SurfaceRef {
            data: &pixels[at..at + len],
            width,
            height,
            depth: 1,
            stride: width * 4,
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
    };
    let image = ImageRef {
        surfaces: vec![surfaces],
        kind: TextureKind::Texture2D,
        desc: FormatDesc {
            format: input,
            color_space,
            // Though alpha isn't multiplied in: ctt multiplies a straight one into the colors as
            // it reads them and divides it out as it hands them to the encoder, which blackens a
            // transparent texel's color, which a run's filtering mixes into those beside it as
            // its levels show, where a development run's texture keeps it. Taken as opaque, the
            // texels go to the encoder as they are, which compresses alpha as it does the colors.
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

#[cfg(test)]
mod tests {
    use gg_engine::cook::{Format as ImageFormat, decode_ktx2};

    use super::*;

    /// The texels of a level's `blocks`, BC7 or BC5 as `compressed` says, `size` texels across and
    /// down, each block decoded as a GPU decodes it: RGBA8, row after row, top row first, BC5's
    /// red and green alone, blue 0 and alpha 255.
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

    /// A texture compresses to the KTX2 a run reads back, of what it holds: BC7 that decodes to its
    /// colors, the color of a transparent texel among them, as a development run's texture keeps
    /// it, or to its values; or BC5 that decodes to a normal map's X and Y.
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
                };
                assert!(
                    close(got, expected),
                    "{compressed:?}'s texel {i}: {got:?}, not {expected:?}"
                );
            }
        }
    }

    /// A texture's full chain of levels compresses to the KTX2 a run reads back, each level's
    /// blocks where a run looks for them, largest first, of that level's colors, every time to the
    /// same bytes.
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
