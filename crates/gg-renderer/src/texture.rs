//! Textures: images that shaders sample, games' and the engine's white one, whose handles are their
//! places in the bindless set, so that a shader's parameters carry them as they are.

use bytemuck::NoUninit;
use gg_core::{Error, Result};
use gg_gpu::{MAX_TEXTURE_SIZE, PLACED_TEXTURES};

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
    pub(crate) const WHITE: Self = Self { index: 0 };
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
}

impl Textures {
    /// Adds a texture `size` pixels across and down. `pixels` holds its rows of RGBA8, tightly
    /// packed, top row first, as images store them: colors sRGB-encoded, and alpha not multiplied
    /// into them. Shaders sample its colors decoded to linear, with texture coordinates from (0, 0)
    /// at its top left to (1, 1) at its bottom right. `name` names it in errors and GPU captures.
    ///
    /// Fails with `renderer.bad_texture` if a side is 0 or more than 8,192 pixels, the most every
    /// Vulkan 1.4 device takes, or `pixels` doesn't fill it exactly; and with
    /// `renderer.too_many_textures` past the 61,440 textures a renderer holds, the engine's white
    /// one among them.
    pub fn add(&mut self, name: &str, size: [u32; 2], pixels: &[u8]) -> Result<Texture> {
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
            pixels: pixels.to_vec(),
        });
        Ok(Texture { index })
    }
}

impl Default for Textures {
    fn default() -> Self {
        let mut textures = Self {
            added: Vec::new(),
            count: 0,
        };
        let white = textures
            .add("white", [1, 1], &[255; 4])
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
                .add("broken", size, &vec![0; bytes])
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
                .add("a", [2, 1], &[0; 8])
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
        for _ in 1..PLACED_TEXTURES {
            textures.add("a", [1, 1], &[0; 4]).expect("a slot for it");
        }
        let error = textures
            .add("one more", [1, 1], &[0; 4])
            .expect_err("no slot left");
        assert_eq!(error.code(), "renderer.too_many_textures");
    }
}
