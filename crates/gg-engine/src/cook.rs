//! What `tools/cook` takes from the engine to write a game's package, so that what it cooks is what
//! the engine makes of each file as a run loads it: the package's [`Writer`]; a font baked and a
//! shader compiled, as a run's add makes them; a texture's image and a model read as a run reads
//! them, and the mip levels of a model's images made as a frame makes them; and a model as a run
//! from a package reads it, cooked. Not for games.

use std::path::Path;

use gg_core::Result;
pub use gg_renderer::{Lod, Mipmaps, mip_sizes};

pub use crate::gltf::{Imported, Primitive};
pub use crate::image::{Format, Image, decode_ktx2};
use crate::package::Files;
pub use crate::package::{Kind, Writer};

/// Font `name`'s file at `path`, baked as [`Context::add_font`](crate::Context::add_font) bakes it,
/// on the job system's threads, which [`start_jobs`](crate::start_jobs) starts, as a game's package
/// holds it.
///
/// Fails as `add_font` does.
pub fn bake_font(name: &str, path: &Path) -> Result<Vec<u8>> {
    Ok(crate::font::bake(name, path)?.bytes())
}

/// Game shader `name`'s file at `path`, compiled to SPIR-V as
/// [`Context::add_shader`](crate::Context::add_shader) compiles it, as a game's package holds it,
/// with the files it read, its own first, then those it includes. A run checks it against the
/// contract for its types as it adds it.
///
/// Fails as `add_shader` does but for the contract.
pub fn compile_shader(name: &str, path: &Path) -> Result<(Vec<u8>, Vec<std::path::PathBuf>)> {
    gg_renderer::compile_shader(name, path)
}

/// Texture `name`'s file at `path`, a PNG or a JPEG, decoded to RGBA8 as
/// [`Context::add_texture`](crate::Context::add_texture) decodes it.
///
/// Fails as `add_texture` does for a file that doesn't decode.
pub fn read_image(name: &str, path: &Path) -> Result<Image> {
    crate::image::load_image(name, path, &Files::default())
}

/// Model `name`'s file at `path`, and the files it names, read as
/// [`Context::add_model`](crate::Context::add_model) reads them, its images decoded to RGBA8 and its
/// primitives of one level.
///
/// Fails as `add_model` does for a model that doesn't read.
pub fn read_model(name: &str, path: &Path) -> Result<Imported> {
    let (imported, _) = crate::gltf::read_listing(name, path, &Files::default())?;
    Ok(imported)
}

/// How image `image` of `model` gets its mip levels, as a run gives them: weighted by alpha where a
/// masked or blended material samples it, and plainly averaged where opaque ones alone do.
pub fn mipmaps(model: &Imported, image: usize) -> Mipmaps {
    crate::model::mipmaps(&model.primitives, image)
}

/// `image`'s RGBA8, then the rest of its full chain of mip levels, smallest last, made as
/// `mipmaps` says, as a frame makes a texture's that the cooker hasn't made; its first level alone
/// for [`Mipmaps::None`].
///
/// # Panics
///
/// If `image` isn't RGBA8.
pub fn levels(image: &Image, mipmaps: Mipmaps) -> Vec<u8> {
    assert_eq!(image.format, Format::Rgba8, "levels are made from RGBA8");
    let mut pixels = image.pixels.clone();
    gg_renderer::add_mipmaps(&mut pixels, image.size, mipmaps);
    pixels
}

/// `model` as a game's package holds it, cooked, as a run from the package reads it in place of
/// its files: its images given as `images`, each one's KTX2, in their order, in place of those
/// `model` has.
///
/// # Panics
///
/// If `images` aren't as many as `model`'s.
pub fn model_bytes(model: &Imported, images: &[Vec<u8>]) -> Vec<u8> {
    crate::cooked::encode(model, images)
}

/// Model `name`'s cooked `bytes`, read back as a run from a game's package reads them, its images
/// read as BC7, for checking what the cooker wrote.
///
/// Fails with `engine.bad_model` for bytes that don't read as a cooked model.
pub fn read_cooked_model(name: &str, bytes: &[u8]) -> Result<Imported> {
    crate::cooked::decode(name, Path::new(name), bytes)
}
