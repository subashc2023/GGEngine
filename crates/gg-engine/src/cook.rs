//! What `tools/cook` takes from the engine to write a game's package, so that what it cooks is what
//! the engine makes of each file as a run loads it: the package's [`Writer`]; a font baked and a
//! shader compiled, as a run's add makes them; a texture's image and a model read as a run reads
//! them, and the mip levels of a model's images made as a frame makes them; and a model as a run
//! from a package reads it, cooked. Not for games.

use std::path::Path;

use gg_core::Result;
pub use gg_renderer::{
    Lod, MAX_MESHLET_TRIANGLES, MAX_MESHLET_VERTICES, Map, Meshlet, Meshlets, Mipmaps, ModelMesh,
    ModelVertex, Texels, mip_sizes,
};

pub use crate::gltf::{Imported, Primitive};
pub use crate::image::{Compressed, Format, Image, decode_ktx2};
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
/// [`Context::add_texture`](crate::Context::add_texture) decodes it, its header and its pixels
/// alike, so that a package holds no texture whose pixels a run would warn of.
///
/// Fails as `add_texture` does for a file whose header doesn't read, and with
/// `engine.bad_texture` for one whose pixels don't decode, of which a run warns.
pub fn read_image(name: &str, path: &Path) -> Result<Image> {
    let image = crate::image::load_image(name, path, &Files::default())?;
    crate::image::decode_texture(name, path, image)
}

/// Model `name`'s file at `path`, and the files it names, read as
/// [`Context::add_model`](crate::Context::add_model) reads them, its images decoded to RGBA8 and its
/// primitives of one level.
///
/// Fails as `add_model` does for a model that doesn't read, and with `engine.bad_model` for one
/// whose image doesn't decode, of which a run warns.
pub fn read_model(name: &str, path: &Path) -> Result<Imported> {
    let (mut imported, _) = crate::gltf::read_listing(name, path, &Files::default())?;
    imported.decode_images(name, path)?;
    Ok(imported)
}

/// What image `image` of `model` holds, and so how it gets its mip levels and how it's compressed,
/// as a run gives them: a map of data, its levels averaging its values; or colors, weighted by alpha
/// where a masked or blended material samples them, and plainly averaged where opaque ones alone
/// do.
pub fn texels(model: &Imported, image: usize) -> Texels {
    crate::model::texels(&model.primitives, image)
}

/// `primitive`'s vertices, as glTF has them, packed as a run packs them, with its indices, of
/// `levels` among them, as a game's package holds them; without meshlets, which the cooker cuts.
pub fn pack(primitive: &Primitive, levels: Vec<Lod>) -> ModelMesh {
    crate::model::pack(primitive, levels)
}

/// `image`'s RGBA8, then the rest of its full chain of mip levels, smallest last, made as `texels`
/// say, as a frame makes a texture's that the cooker hasn't made; its first level alone for
/// [`Mipmaps::None`].
///
/// # Panics
///
/// If `image` isn't RGBA8.
pub fn levels(image: &Image, texels: Texels) -> Vec<u8> {
    assert_eq!(image.format, Format::Rgba8, "levels are made from RGBA8");
    let mut pixels = image.pixels.clone();
    gg_renderer::add_levels(&mut pixels, image.size, texels);
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
/// decoded to BC7 as the loader decodes them, for checking what the cooker wrote.
///
/// Fails with `engine.bad_model` for bytes that don't read as a cooked model, or whose image
/// doesn't decode.
pub fn read_cooked_model(name: &str, bytes: &[u8]) -> Result<Imported> {
    let path = Path::new(name);
    let mut imported = crate::cooked::decode(name, path, bytes)?;
    imported.decode_images(name, path)?;
    Ok(imported)
}
