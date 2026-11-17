//! What `tools/cook` takes from the engine to write a game's package, so its output is what a run
//! makes of each file: the package's [`Writer`], and fonts, shaders, textures, models and mip
//! levels made, and cooked models read, as a run does; and [`in_order`], to make them on the job
//! system's threads. Not for games.

use std::path::Path;

use gg_core::Result;
pub use gg_renderer::{
    Lod, MAX_MESHLET_TRIANGLES, MAX_MESHLET_VERTICES, Map, Meshlet, Meshlets, Mipmaps, ModelMesh,
    ModelVertex, Texels, mip_sizes,
};

pub use crate::gltf::{Imported, Primitive};
pub use crate::image::{Compressed, Format, Image, decode_ktx2};
pub use crate::jobs::in_order;
use crate::package::Files;
pub use crate::package::{Kind, Writer};

/// Font `name`'s file at `path`, baked as [`Context::add_font`](crate::Context::add_font) bakes
/// it, on the threads [`jobs::start`](crate::jobs::start) starts.
///
/// Fails as `add_font` does.
pub fn bake_font(name: &str, path: &Path) -> Result<Vec<u8>> {
    Ok(crate::font::bake(name, path)?.bytes())
}

/// What a UI's node names; a run loads it with the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAsset {
    Font,
    Texture,
}

/// UI `name`'s file at `path`, read and checked as [`Context::add_ui`](crate::Context::add_ui)
/// reads it.
///
/// That is the file as is, and each font and texture its nodes name, once, as named: by asset ID (a
/// model's image by model name and index, `5f3c9a1e07b2d846#0`) or by path from the asset root. The
/// cooker finds them in the package as a run would, so it holds no UI a run refuses.
///
/// Fails as `add_ui` does for a file that doesn't read or isn't a UI's.
pub fn read_ui(name: &str, path: &Path) -> Result<ReadUi> {
    let document = crate::game_ui::read(name, path, &Files::default())?;
    let mut named: Vec<(UiAsset, String)> = Vec::new();
    for node in &document.nodes {
        let fonts = node.font.iter().map(|font| (UiAsset::Font, &font.name));
        let textures = node.looks.iter().filter_map(|look| look.texture.as_ref());
        let textures = textures.map(|texture| (UiAsset::Texture, &texture.name));
        for (kind, asset) in fonts.chain(textures) {
            if !named
                .iter()
                .any(|(was, at)| *was == kind && **at == **asset)
            {
                named.push((kind, asset.to_string()));
            }
        }
    }
    let bytes = std::fs::read(path).map_err(|error| {
        let message = format!("can't read UI {name:?} from {}", path.display());
        gg_core::Error::new("engine.bad_ui", message).with_source(error)
    })?;
    Ok(ReadUi { bytes, named })
}

/// A UI's file as [`read_ui`] reads it: bytes and the names in its nodes.
pub struct ReadUi {
    pub bytes: Vec<u8>,
    pub named: Vec<(UiAsset, String)>,
}

/// Whether `name` is an asset's ID, 16 lowercase hex digits, as a run tells one from a path.
pub fn is_id(name: &str) -> bool {
    crate::assets::is_id(name)
}

/// The package entry name for `name`, a path from asset root `root`: parts joined by `/`, `.` and
/// `..` resolved as written. `None` for an absolute path or one outside the root.
pub fn within(root: &Path, name: &str) -> Option<String> {
    let path = Path::new(name);
    let prefixed = matches!(
        path.components().next(),
        Some(std::path::Component::Prefix(_))
    );
    if path.has_root() || prefixed {
        return None;
    }
    crate::package::within(root, &root.join(path))
}

/// Game shader `name`'s file at `path`, compiled to SPIR-V as
/// [`Context::add_shader`](crate::Context::add_shader) does, with the files it read: its own
/// first, then its includes. Unlike a run, this skips the types' contract check.
///
/// Fails as `add_shader` does, but for the contract.
pub fn compile_shader(name: &str, path: &Path) -> Result<(Vec<u8>, Vec<std::path::PathBuf>)> {
    gg_renderer::compile_shader(name, path)
}

/// Texture `name`'s file at `path`, a PNG or JPEG, decoded to RGBA8 as
/// [`Context::add_texture`](crate::Context::add_texture) does, pixels included, so a package
/// holds no texture a run would warn of.
///
/// Fails as `add_texture` does for an unreadable header, and with `engine.bad_texture` for
/// pixels that don't decode (a run warns).
pub fn read_image(name: &str, path: &Path) -> Result<Image> {
    let image = crate::image::load_image(name, path, &Files::default())?;
    crate::image::decode_texture(name, path, image)
}

/// Model `name`'s file at `path` and the files it names, read as
/// [`Context::add_model`](crate::Context::add_model) does: images in RGBA8, primitives of one
/// level.
///
/// Fails as `add_model` does for an unreadable model, and with `engine.bad_model` for an image
/// that doesn't decode (a run warns).
pub fn read_model(name: &str, path: &Path) -> Result<Imported> {
    let (mut imported, _) = crate::gltf::read_listing(name, path, &Files::default())?;
    imported.decode_images(name, path)?;
    Ok(imported)
}

/// What image `image` of `model` holds, which decides its mips and compression as in a run.
///
/// It is either a data map (averaged levels), or colors (alpha-weighted if a masked or blended
/// material samples them, else plainly averaged).
pub fn texels(model: &Imported, image: usize) -> Texels {
    crate::model::texels(&model.primitives, image)
}

/// Model `name`'s `primitive`: glTF vertices packed as a run packs them, with indices (`levels`
/// among them). Meshlets are left to the cooker.
///
/// Fails with `engine.bad_model`, as `add_model` does, for vertices that don't pack.
pub fn pack(name: &str, primitive: &Primitive, levels: Vec<Lod>) -> Result<ModelMesh> {
    crate::model::pack(name, primitive, levels)
}

/// `image`'s RGBA8 then the rest of its mip chain, smallest last, made as `texels` say and as a
/// frame makes a texture's the cooker hasn't. [`Mipmaps::None`] gives the first level alone.
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

/// `model` (named `name`) cooked as a run from a package reads it instead of its files:
/// `images` (each one's KTX2, in order) replace `model`'s, and each primitive's mesh is as
/// [`pack`] packed and the cooker cut it.
///
/// Fails with `engine.bad_model`, as `add_model` does, for a mesh that doesn't hold together.
///
/// # Panics
///
/// - `images` aren't as many as `model`'s.
/// - A primitive has no mesh.
pub fn model_bytes(name: &str, model: &Imported, images: &[Vec<u8>]) -> Result<Vec<u8>> {
    for primitive in &model.primitives {
        let mesh =
            (primitive.mesh.as_ref()).expect("a cooked model's primitives have their meshes");
        crate::model::check_mesh(name, primitive, mesh)?;
    }
    Ok(crate::cooked::encode(model, images))
}

/// Model `name`'s cooked `bytes` read back as a run does (images decoded to BC7), to check what
/// the cooker wrote.
///
/// Fails with `engine.bad_model` for bytes that aren't a cooked model or whose image doesn't
/// decode.
pub fn read_cooked_model(name: &str, bytes: &[u8]) -> Result<Imported> {
    let path = Path::new(name);
    let mut imported = crate::cooked::decode(name, path, bytes)?;
    imported.decode_images(name, path)?;
    Ok(imported)
}
