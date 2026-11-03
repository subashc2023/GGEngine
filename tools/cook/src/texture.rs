//! Textures compressed to BC7, in KTX2, as a game's package holds them: a layer's texture of one
//! level, and a model's image with its full chain of mip levels, which the engine makes for the
//! cooker as a frame makes a texture's.

use ctt::encoders::Encoder;
use ctt::{
    AlphaMode, ColorSpace, Container, ConvertSettings, Format, FormatDesc, ImageRef,
    PipelineOutput, Quality, SurfaceRef, TargetFormat, TextureKind,
};
use gg_engine::cook::mip_sizes;
use gg_engine::{Error, Result};

/// The zstd level each of a texture's levels is supercompressed at: zstd's highest short of its
/// ultra levels, which take far more memory to compress. BC7's blocks shrink little whatever the
/// level, but reading them back takes no longer for a higher one.
const ZSTD_LEVEL: i32 = 19;

/// Texture `name`'s `pixels`, rows of RGBA8, sRGB-encoded and alpha not multiplied in, `size`
/// texels across and down, followed, if `mipmapped`, by the rest of their full chain of levels,
/// smallest last: compressed to BC7 by bc7enc_rdo's perceptual encoder, each level supercompressed
/// by zstd, in KTX2, as [`decode_ktx2`](gg_engine::cook::decode_ktx2) reads it. The same pixels
/// compress to the same bytes every time on one machine.
///
/// Fails with `cook.uncompressed` if ctt can't compress them.
///
/// # Panics
///
/// If `pixels` don't fill the levels exactly.
pub(crate) fn bc7(name: &str, size: [u32; 2], pixels: &[u8], mipmapped: bool) -> Result<Vec<u8>> {
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
    let image = ImageRef {
        surfaces: vec![surfaces],
        kind: TextureKind::Texture2D,
        desc: FormatDesc {
            format: Format::R8G8B8A8_SRGB,
            color_space: ColorSpace::Srgb,
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
            format: Format::BC7_SRGB_BLOCK,
            encoder: Encoder::Auto,
        }),
        container: Container::ktx2_zstd(ZSTD_LEVEL),
        quality: Quality::Basic,
        // Its levels are the engine's, as given.
        mipmap: false,
        ..ConvertSettings::default()
    };
    let uncompressed = |problem: String| {
        let message = format!("texture {name:?} doesn't compress to BC7: {problem}");
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

    /// The texels of a level's BC7 `blocks`, `size` texels across and down, each block decoded as a
    /// GPU decodes it: RGBA8, row after row, top row first.
    fn decoded(blocks: &[u8], [width, height]: [u32; 2]) -> Vec<[u8; 4]> {
        let (width, height) = (width as usize, height as usize);
        let across = width.div_ceil(4);
        let mut texels = vec![[0; 4]; width * height];
        for (i, block) in blocks.as_chunks::<16>().0.iter().enumerate() {
            let mut decoded = [0; 64];
            bcdec_rs::bc7(block, &mut decoded, 16);
            let (left, top) = (i % across * 4, i / across * 4);
            for (j, texel) in decoded.as_chunks::<4>().0.iter().enumerate() {
                let (x, y) = (left + j % 4, top + j / 4);
                // A block that covers the level's edge holds texels past it.
                if x < width && y < height {
                    texels[y * width + x] = *texel;
                }
            }
        }
        texels
    }

    /// Whether each channel of `texel` is within BC7's loss of `expected`'s.
    fn close(texel: [u8; 4], expected: [u8; 4]) -> bool {
        (texel.iter().zip(expected)).all(|(&got, want)| got.abs_diff(want) <= 3)
    }

    /// A texture compresses to the KTX2 a run reads back, of BC7 that decodes to its colors, the
    /// color of a transparent texel among them, as a development run's texture keeps it.
    #[test]
    fn a_texture_compresses_to_bc7_of_its_colors() {
        // 8x4 texels: opaque on the left, transparent but colored on the right.
        let (opaque, clear) = ([201, 41, 91, 255], [31, 201, 61, 0]);
        let texel = |i: usize| if i % 8 < 4 { opaque } else { clear };
        let pixels: Vec<u8> = (0..32).flat_map(texel).collect();
        let ktx2 = bc7("halves", [8, 4], &pixels, false).expect("compressed");
        let image = decode_ktx2(&ktx2).expect("KTX2 a run reads");
        assert_eq!(image.size, [8, 4]);
        assert_eq!(image.format, ImageFormat::Bc7 { mipmapped: false });
        for (i, got) in decoded(&image.pixels, [8, 4]).into_iter().enumerate() {
            assert!(
                close(got, texel(i)),
                "texel {i}: {got:?}, not {:?}",
                texel(i)
            );
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
        let chain = bc7("levels", [13, 5], &pixels, true).expect("compressed");
        let image = decode_ktx2(&chain).expect("KTX2 a run reads");
        assert_eq!(image.format, ImageFormat::Bc7 { mipmapped: true });
        let mut at = 0;
        for (level, (&size, color)) in sizes.iter().zip(colors).enumerate() {
            let len = (size[0].div_ceil(4) * size[1].div_ceil(4) * 16) as usize;
            let texels = decoded(&image.pixels[at..at + len], size);
            assert!(
                texels.iter().all(|&got| close(got, color)),
                "level {level}: {texels:?}"
            );
            at += len;
        }
        assert_eq!(at, image.pixels.len(), "every level's blocks, and no more");
        let again = bc7("levels", [13, 5], &pixels, true).expect("compressed");
        assert_eq!(again, chain, "the same bytes");
    }
}
