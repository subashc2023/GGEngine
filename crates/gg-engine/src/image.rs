//! Images that layers load from disk as textures: PNG files, decoded to the RGBA8 that the
//! renderer takes.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek};
use std::path::Path;

use gg_core::{Error, Result};

/// An image's size in pixels, and its rows of RGBA8, tightly packed, top row first, as its file
/// stores them: colors sRGB-encoded, and alpha not multiplied into them.
pub(crate) struct Image {
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Vec<u8>,
}

/// Reads the PNG file at `path`, which is texture `name`'s.
///
/// Fails with `engine.bad_texture` if the file can't be read, or isn't a PNG that decodes.
pub(crate) fn load_png(name: &str, path: &Path) -> Result<Image> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bad = |problem: String| {
        Error::new(
            "engine.bad_texture",
            format!("texture {name:?} at {} {problem}", path.display()),
        )
    };
    let file = File::open(path)
        .map_err(|error| bad(format!("can't be read: {error}")).with_source(error))?;
    decode_png(BufReader::new(file))
        .map_err(|error| bad(format!("doesn't decode as a PNG: {error}")).with_source(error))
}

/// Decodes a PNG of any color type and bit depth to RGBA8: gray to equal red, green and blue, a
/// palette to its colors, 16-bit samples to their high bytes, and missing alpha to opaque.
fn decode_png(file: impl BufRead + Seek) -> Result<Image, png::DecodingError> {
    let mut decoder = png::Decoder::new(file);
    // Whatever the file holds, RGBA or gray with alpha, 8 bits a sample.
    decoder.set_transformations(png::Transformations::ALPHA | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let size = reader
        .output_buffer_size()
        .ok_or(png::DecodingError::LimitsExceeded)?;
    let mut decoded = vec![0; size];
    let info = reader.next_frame(&mut decoded)?;
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
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// A PNG one row of `pixels` high, of `width` pixels in `color` and `depth`, with the palette
    /// and transparency chunks given.
    fn png(
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
                // The first color transparent; the second, without an alpha of its own, opaque.
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
            let image = decode_png(Cursor::new(file)).expect("a PNG");
            assert_eq!(image.size, [2, 1], "{kind}");
            assert_eq!(image.pixels, rgba, "{kind}");
        }
    }

    #[test]
    fn a_texture_that_doesnt_load_names_itself_and_its_file() {
        let missing = Path::new("no such folder/checkerboard.png");
        let not_a_png = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        for path in [missing, &not_a_png] {
            let error = load_png("checkerboard", path).err().expect("no image");
            assert_eq!(error.code(), "engine.bad_texture", "{}", path.display());
            let message = error.to_string();
            assert!(
                message.contains("\"checkerboard\"")
                    && message.contains(&path.display().to_string()),
                "{message}"
            );
        }
    }
}
