//! Images that layers load from disk as textures, and models' images: PNGs and JPEGs, decoded to
//! the RGBA8 that the renderer takes.

use std::io::Cursor;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use gg_core::{Error, Result};
use gg_renderer::MAX_TEXTURE_SIZE;
use zune_jpeg::JpegDecoder;
use zune_jpeg::errors::DecodeErrors;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::package::Files;

/// The most bytes an image's file may hold: more than a PNG or a JPEG of a texture's size needs, as
/// 8,192 pixels square of 16-bit RGBA, stored uncompressed, are 512 MiB.
pub(crate) const MAX_FILE: u64 = 1 << 30;

/// An image's size in pixels, and its rows of RGBA8, tightly packed, top row first, as its file
/// stores them: colors sRGB-encoded, and alpha not multiplied into them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Image {
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Vec<u8>,
}

/// Reads the image file at `path` from `files`, which is texture `name`'s: a PNG or a JPEG.
///
/// Fails with `engine.bad_texture` if the file can't be read or isn't a regular file, or doesn't
/// decode as either.
pub(crate) fn load_image(name: &str, path: &Path, files: &Files) -> Result<Image> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bad = |problem: String| {
        Error::new(
            "engine.bad_texture",
            format!("texture {name:?} at {} {problem}", path.display()),
        )
    };
    // A byte past the most an image's file holds, so that a longer one fails to decode.
    let bytes = (files.read(path, MAX_FILE + 1))
        .map_err(|error| bad("can't be read".to_owned()).with_source(error))?;
    decode(&bytes).map_err(|problem| bad(format!("doesn't decode as a PNG or a JPEG: {problem}")))
}

/// Decodes a PNG or a JPEG, which it tells apart by their first bytes, whatever a file's name or a
/// model says it is, to RGBA8. Color profiles and gamma are ignored, as glTF requires, so colors are
/// taken as sRGB-encoded. The error says what's wrong with the image.
///
/// Fails before anything is allocated for the pixels if a side is 0 or more than
/// [`MAX_TEXTURE_SIZE`], since no texture can be, and if there are more than [`MAX_FILE`] bytes.
pub(crate) fn decode(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(format!("it's more than {} MiB", MAX_FILE >> 20));
    }
    let decode = || {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            decode_png(bytes)
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            decode_jpeg(bytes)
        } else {
            Err("it starts with neither a PNG's signature nor a JPEG's".to_owned())
        }
    };
    // zune-jpeg can panic on a malformed file, as its README allows, and png only by a bug: either
    // way that's the file's doing, not the engine's, so the image fails as one that doesn't decode
    // does. gg-core's panic hook still logs the panic as it happens.
    panic::catch_unwind(AssertUnwindSafe(decode))
        .unwrap_or_else(|_| Err("its decoder panicked on it".to_owned()))
}

/// Fails unless each side is 1 to [`MAX_TEXTURE_SIZE`] pixels, as every texture's is.
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

/// Decodes a PNG of any color type and bit depth to RGBA8: gray to equal red, green and blue, a
/// palette to its colors, 16-bit samples to their high bytes, and missing alpha to opaque.
fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let png = |error: png::DecodingError| error.to_string();
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // Whatever the file holds, RGBA or gray with alpha, 8 bits a sample.
    decoder.set_transformations(png::Transformations::ALPHA | png::Transformations::STRIP_16);
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
    })
}

/// Decodes a JPEG that's Huffman-coded at 8 bits, sequential or progressive, to RGBA8: gray to
/// equal red, green and blue, YCbCr, CMYK and YCCK converted, CMYK taken as Adobe stores it,
/// inverted, RGB as it is, and alpha opaque.
fn decode_jpeg(bytes: &[u8]) -> Result<Image, String> {
    let jpeg_error = |error: DecodeErrors| error.to_string();
    let decoder = |colorspace| {
        let options = DecoderOptions::default()
            .jpeg_set_out_colorspace(colorspace)
            // So that it refuses a larger image as it reads the header, before it allocates.
            .set_max_width(MAX_TEXTURE_SIZE as usize)
            .set_max_height(MAX_TEXTURE_SIZE as usize)
            // So that a scan cut short or corrupt fails rather than ending in gray.
            .set_strict_mode(true);
        JpegDecoder::new_with_options(ZCursor::new(bytes), options)
    };
    let mut jpeg = decoder(ColorSpace::RGBA);
    jpeg.decode_headers().map_err(jpeg_error)?;
    // zune-jpeg (0.5.15) makes RGB rightly from every color space it reads, but RGBA from gray and
    // YCbCr alone, scrambling CMYK's and YCCK's and refusing RGB's; and it picks how it converts as
    // it reads the headers. So the rest decode afresh, to RGB, which gets its alpha below. Its
    // 0.5.16 release candidates make RGBA rightly from CMYK and YCCK, though not from RGB.
    let rgb = !matches!(
        jpeg.input_colorspace(),
        Some(ColorSpace::Luma | ColorSpace::YCbCr)
    );
    if rgb {
        jpeg = decoder(ColorSpace::RGB);
    }
    let decoded = jpeg.decode().map_err(jpeg_error)?;
    // Strict mode can miss a sequential scan cut in its last row of MCUs, whose blocks the zeros
    // zune-jpeg reads past the end finish. A file that ends in an end-of-image marker wasn't cut
    // in a scan, whose data holds none; a file that doesn't is walked to its marker, if it has one.
    if !bytes.ends_with(&[0xff, 0xd9]) && !reaches_its_end(bytes) {
        return Err("it ends before its end-of-image marker".to_owned());
    }
    let (Some(info), Some(colorspace)) = (jpeg.info(), jpeg.input_colorspace()) else {
        unreachable!("a JPEG that decodes has had its header read");
    };
    // zune-jpeg settles some color spaces only as it decodes, such as RGB from an Adobe marker's
    // CMYK of three components. Strict mode refuses fewer components than the one it settles on
    // takes, but takes too few for YCCK as YCbCr, rightly for three; fewer than that, and more
    // than it takes, such as four that a later Adobe marker calls YCbCr, it decodes to the wrong
    // pixels rather than failing.
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
    })
}

/// Whether a JPEG's segments, each as long as it says, and its scans' data, which holds no marker
/// but restarts, run from its start to its end-of-image marker, which a file cut short has lost.
/// What follows that marker, such as a motion photo's video, isn't read.
fn reaches_its_end(bytes: &[u8]) -> bool {
    // A marker, not a scan's 0xff, which has a 0 stuffed after it, a restart or a fill byte.
    let marker = |pair: &[u8]| pair[0] == 0xff && !matches!(pair[1], 0 | 0xd0..=0xd7 | 0xff);
    // Past its start-of-image marker.
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
    use super::*;

    /// A PNG one row of `pixels` high, of `width` pixels in `color` and `depth`, with the palette
    /// and transparency chunks given.
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
        // Why it can't be read, once: as what caused the error, and not in its message too.
        let error = load_image("checkerboard", missing, &Files::default()).expect_err("no image");
        let shown = format!("{error:#}");
        assert_eq!(shown.matches("(os error").count(), 1, "{shown}");
    }

    /// A 16x16 baseline JPEG, as Windows' encoder writes one at quality 100, of four 8x8 quadrants
    /// of `QUADRANTS`' colors: the top left, the top right, the bottom left and the bottom right.
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

    /// A 16x16 baseline JPEG of CMYK, inverted as Adobe stores it, as libjpeg-turbo writes one
    /// through Pillow at quality 100, of four 8x8 quadrants: `QUADRANTS`' colors as ink, with black
    /// ink of 0, 20, 40 and 60%, which make `INKED`.
    fn cmyk() -> Vec<u8> {
        let base64 = concat!(
            "/9j/7gAOQWRvYmUAZAAAAAAA/9sAQwABAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
            "AQEBAQEBAQEBAQEBAQEBAQEB/8AAFAgAEAAQBEMRAE0RAFkRAEsRAP/EABgAAQEBAQEAAAAAAAAAAAAAAAoLCQcI/8QA",
            "FBABAAAAAAAAAAAAAAAAAAAAAP/aAA4EQwBNAFkASwAAPwBIAn4n5/CX+eA9QMZ3UBL1BBjOrQKMAR7Gd//Z",
        );
        crate::gltf::base64(base64).expect("base64")
    }

    /// The image `cmyk` holds, as Windows' encoder writes CMYK at quality 100: as YCCK, its cyan,
    /// magenta and yellow as YCbCr, subsampled, beside its black.
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

    /// The YCbCr that a JPEG holds for `color`, by JFIF's formulas.
    fn ycbcr(color: [u8; 3]) -> [u8; 3] {
        let [r, g, b] = color.map(f32::from);
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        let cb = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
        let cr = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
        [y, cb, cr].map(|channel| channel.round() as u8)
    }

    #[test]
    fn a_jpeg_decodes_to_rgba8() {
        // The JPEG, and the JPEG with its components named R, G and B, as some encoders name those
        // of a JPEG that holds RGB, so that its YCbCr decodes as RGB, as it is; and the JPEG with
        // an Adobe marker of transform 0 instead, which says the same of three components. And the
        // JPEG followed by what looks like a scan's header, as a motion photo's video can hold.
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
            // JPEG loses a little: each quadrant, but for the pixels at its edges, which blur into
            // the next, is within 2 of its color, and opaque.
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
        // `cmyk` with its frame and its scan cut to their first two components, too few for its
        // CMYK, which zune-jpeg refuses; and `cmyk` with its Adobe marker moved after its frame
        // header and saying YCbCr, which zune-jpeg decodes its four components as.
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
        // Its Adobe marker is its first, whose last byte, its transform, says what its components
        // are; and its frame header ends where its Huffman tables start.
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
        // A PNG 9000 pixels across, which would decode; and the JPEG with its header changed to
        // say it's 9000 across, which zune-jpeg refuses as it reads it.
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
            // `cmyk`'s scan cut in the first of two rows of MCUs, which zune-jpeg sees ran short as
            // the second starts; and `jpeg`'s in its only row, which only its lost end shows.
            ("a JPEG's scan cut short", &cmyk[..cmyk.len() - 20]),
            ("a JPEG's last row cut short", &jpeg[..jpeg.len() - 20]),
        ];
        for (kind, bytes) in cases {
            assert!(decode(bytes).is_err(), "{kind}");
        }
    }
}
