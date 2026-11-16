//! glTF 2.0 models: reads a model's `.gltf` or `.glb` file, and the buffers and images it names,
//! into what its default scene draws, for the engine to add as entities, meshes and textures.
//! Reading adds nothing anywhere, so a model that fails to read leaves no trace.

mod animation;
mod buffers;
/// The parts of a glTF file's JSON that reading uses, which serde reads with glTF's defaults,
/// skipping the rest, extras and extensions among them. Every list is in the file's order, and
/// attributes are by name, so nothing follows a hash's order.
mod json;
mod material;
mod mesh;
mod scene;

#[cfg(test)]
pub(crate) use buffers::base64;
#[cfg(feature = "dev")]
pub(crate) use material::holds;
pub(crate) use material::{Material, Sampling, Slot};
pub use mesh::Primitive;
pub(crate) use scene::{allowed, takes};

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::io;
use std::mem;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gg_core::{Error, Result};
use gg_renderer::Map;

use crate::animation::{Clip, Skin};
#[cfg(doc)]
use crate::image;
use crate::image::Image;
use crate::package::Files;
use crate::{Light, Transform};

/// What `read` gives: the default scene's parts, the primitives they draw and the images those
/// sample. The cooker reads it through [`cook`](crate::cook), and cooks its primitives and images.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    /// The scene's nodes, and a child for each primitive of a mesh with more than one, in
    /// pre-order: each part after its parent.
    pub(crate) parts: Vec<Part>,
    /// Each primitive the scene draws, once, in the order the walk first reaches it.
    pub primitives: Vec<Primitive>,
    /// Each image a reached material samples, once, in the order first reached: read as far as its
    /// header, for the loader to decode, unless the engine has decoded it, as for hot reload.
    pub images: Vec<Image>,
    /// Each skin a part's node names, once, in the order the walk first reaches it.
    pub(crate) skins: Vec<Skin>,
    /// Each animation, in the file's order, its channels of nodes in the scene.
    pub(crate) clips: Vec<Clip>,
    /// What the file has that the engine ignores, for an info event, such as "1 camera" or
    /// "extension KHR_texture_transform".
    pub(crate) ignored: Vec<String>,
}

impl Imported {
    /// Decodes its images, which reading it read as far as their headers, as [`Image::decoded`]
    /// decodes them, model `name`'s from `path`: as a model read again, which hot reload puts in
    /// place at once, and the cooker need them.
    ///
    /// Fails with `engine.bad_model`, naming the image as the model's textures are named, for one
    /// that doesn't decode.
    pub(crate) fn decode_images(&mut self, name: &str, path: &Path) -> Result<()> {
        let images = mem::take(&mut self.images).into_iter().enumerate();
        let decoded = images.map(|(i, image)| {
            let model = Model { name, path };
            let decoded = image.decoded();
            decoded.map_err(|problem| {
                model.bad(format!("has image {i}, which doesn't decode: {problem}"))
            })
        });
        self.images = decoded.collect::<Result<_>>()?;
        Ok(())
    }
}

/// A node of the scene, or a primitive of a node's mesh: what an entity holds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Part {
    /// Its node's name, as the glTF gives it, if it isn't empty: what the world calls the node,
    /// as [`World::name`](crate::World::name) says. None for a primitive's part.
    pub(crate) name: Option<Arc<str>>,
    /// Where it is in its parent's space, or in the model's if it has none.
    pub(crate) transform: Transform,
    /// Its parent's index in `parts`, which comes before its own.
    pub(crate) parent: Option<usize>,
    /// The index in `primitives` of what it draws, if it draws anything.
    pub(crate) primitive: Option<usize>,
    /// The light its node holds, as KHR_lights_punctual gives it, if it has one.
    pub(crate) light: Option<Light>,
    /// The index in `skins` of the skin whose joints move what it draws, if its node's skinned.
    pub(crate) skin: Option<usize>,
}

/// The most memory reading a model may take, in bytes, for what it makes of its files: buffers read
/// from files and data URIs, images' files and what they decode to, which both are held until the
/// loader has decoded them, primitives' vertices, indices and names, and parts; and while it reads,
/// the accessors it has read, and a primitive's vertices before it's drawn flat or its vertices
/// part for MikkTSpace's tangents, with what making those takes.
/// Past what a model that fits a GPU needs, as the Amazon Lumberyard Bistro's 2 GB of images, BC7
/// with their levels, held as their files and as their blocks, and its 4 million triangles take
/// about 4.2 GiB at most as it's read, so that a small file can't fill memory by naming its data
/// many times, such as one PNG's bytes as many images, nor by its primitives' vertices, as each
/// corner of a triangle drawn flat takes a vertex of 64 bytes, though an index of a byte names it.
pub(crate) const MAX_MEMORY: u64 = 8 << 30;

/// Reads model `name`'s file at `path`, and the files it names, from `from`, the disk or the game's
/// package, as [`read`] does through [`Files::read`], and lists the files it read, its own first,
/// each once: those that hot reload watches, and that a package holds for it.
pub(crate) fn read_listing(
    name: &str,
    path: &Path,
    from: &Files,
) -> Result<(Imported, Vec<PathBuf>)> {
    let files: RefCell<Vec<PathBuf>> = RefCell::default();
    let read_file = |file: &Path, most: u64| {
        let mut files = files.borrow_mut();
        if !files.iter().any(|read| read == file) {
            files.push(file.to_owned());
        }
        from.read(file, most)
    };
    let imported = read(name, path, read_file, MAX_MEMORY)?;
    Ok((imported, files.into_inner()))
}

/// Reads model `name`'s file at `path`, and the files it names, through `read_file`, which reads a
/// regular file as far as the bytes it's given ([`dirs::read_file`](crate::dirs::read_file), or
/// tests' files from memory), taking no more than `memory` bytes of what [`MAX_MEMORY`] counts, and
/// taking, ignoring and refusing what [`Context::add_model`](crate::Context::add_model) says. Each
/// check it makes fails with the element it's about, such as "accessor 7 reads past buffer view 2".
pub(crate) fn read(
    name: &str,
    path: &Path,
    read_file: impl Fn(&Path, u64) -> io::Result<Vec<u8>>,
    memory: u64,
) -> Result<Imported> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let model = Model { name, path };
    // As far as a GLB's header can say it reaches, past which a GLB holds nothing, and a .gltf's
    // JSON, cut short, doesn't parse.
    let file = read_file(path, u32::MAX.into())
        .map_err(|error| model.bad("can't be read").with_source(error))?;
    let (json, bin) = container(model, &file)?;
    // A byte-order mark, which JSON may start with.
    let json = json.strip_prefix(b"\xef\xbb\xbf").unwrap_or(json);
    let not_gltf = |error: serde_json::Error| model.bad("isn't glTF's JSON").with_source(error);
    // The version first, since a file of another version may have its other fields of other types.
    let versioned: json::Versioned = serde_json::from_slice(json).map_err(not_gltf)?;
    check_version(model, &versioned.asset)?;
    let gltf: json::Gltf = serde_json::from_slice(json).map_err(not_gltf)?;

    let read = |extension: &&String| READ_EXTENSIONS.contains(&extension.as_str());
    if let Some(extension) = gltf.extensions_required.iter().find(|e| !read(e)) {
        return Err(model.unsupported(format!(
            "requires extension {extension}, which the engine doesn't read"
        )));
    }
    let mut ignored = Vec::new();
    match gltf.cameras.len() {
        0 => {}
        1 => ignored.push("1 camera".to_owned()),
        count => ignored.push(format!("{count} cameras")),
    }
    ignored.extend(
        (gltf.extensions_used.iter())
            .filter(|e| !read(e))
            .map(|extension| format!("extension {extension}")),
    );
    let mut reader = Reader {
        model,
        read_file,
        gltf: &gltf,
        bin,
        buffers: BTreeMap::new(),
        floats: BTreeMap::new(),
        indices: BTreeMap::new(),
        meshes: BTreeMap::new(),
        images: BTreeMap::new(),
        holds: BTreeMap::new(),
        parts_of: Vec::new(),
        imported: Imported {
            ignored,
            ..Imported::default()
        },
        memory: Memory {
            model,
            held: 0,
            most: memory,
        },
    };
    reader.scene()?;
    reader.animations()?;
    reader.reach()?;
    Ok(reader.imported)
}

/// What reading a model holds of what [`MAX_MEMORY`] counts, in bytes, and the most it may.
struct Memory<'a> {
    model: Model<'a>,
    held: u64,
    most: u64,
}

impl Memory<'_> {
    /// Counts `bytes` more held, before they're allocated, failing if that's past the most reading
    /// may take, so that it never holds more; but for what's read of a file or a data URI, counted
    /// once it's read, so that reading takes at most one of those more: a buffer's file, read no
    /// further than a byte past the room left, an image's, of at most [`image::MAX_FILE`], or a
    /// data URI's bytes, fewer than the model's own file holds.
    fn hold(&mut self, bytes: usize) -> Result<()> {
        self.hold_for(bytes, "its buffers, images, vertices and nodes")
    }

    /// Counts `bytes` more held as [`hold`](Self::hold) does, for `what`, which the error names.
    fn hold_for(&mut self, bytes: usize, what: &str) -> Result<()> {
        let held = self.held.saturating_add(bytes as u64);
        if held > self.most {
            return Err(self.model.unsupported(format!(
                "takes more than {} MiB to read, of {what}",
                self.most >> 20
            )));
        }
        self.held = held;
        Ok(())
    }

    /// Counts `bytes` that [`hold`](Self::hold) counted as freed.
    fn release(&mut self, bytes: usize) {
        self.held = self.held.saturating_sub(bytes as u64);
    }

    /// `items`, copied once the copy is counted.
    fn copy<T: Clone>(&mut self, items: &[T]) -> Result<Vec<T>> {
        self.hold(size_of_val(items))?;
        Ok(items.to_vec())
    }

    /// `len` of `item`, once they're counted.
    fn filled<T: Clone>(&mut self, item: T, len: usize) -> Result<Vec<T>> {
        self.hold(len * size_of::<T>())?;
        Ok(vec![item; len])
    }
}

/// The extensions the engine reads: materials drawn unlit, nodes' lights, materials' emission
/// stronger than 1, materials of specular and glossiness, and textures' DDS images, which it
/// samples in place of their sources.
const READ_EXTENSIONS: [&str; 5] = [
    "KHR_materials_unlit",
    "KHR_lights_punctual",
    "KHR_materials_emissive_strength",
    "KHR_materials_pbrSpecularGlossiness",
    "MSFT_texture_dds",
];

const GLB_MAGIC: u32 = u32::from_le_bytes(*b"glTF");
const JSON_CHUNK: u32 = u32::from_le_bytes(*b"JSON");
const BIN_CHUNK: u32 = u32::from_le_bytes(*b"BIN\0");

/// The model that errors name, with its file.
#[derive(Clone, Copy)]
struct Model<'a> {
    name: &'a str,
    path: &'a Path,
}

impl Model<'_> {
    fn error(self, code: &'static str, problem: impl Display) -> Error {
        let path = self.path.display();
        Error::new(code, format!("model {:?} at {path} {problem}", self.name))
    }

    /// An `engine.bad_model`, for a file that can't be read or isn't glTF.
    fn bad(self, problem: impl Display) -> Error {
        self.error("engine.bad_model", problem)
    }

    /// An `engine.unsupported_model`, for glTF that the engine doesn't draw.
    fn unsupported(self, problem: impl Display) -> Error {
        self.error("engine.unsupported_model", problem)
    }

    /// `items[index]`, which the file names as its `what` `index`.
    fn item<'i, T>(self, items: &'i [T], what: &str, index: u32) -> Result<&'i T> {
        (items.get(index as usize)).ok_or_else(|| self.bad(format!("has no {what} {index}")))
    }
}

/// Splits `file` into its JSON and its GLB's BIN chunk, if it's a GLB with one; any other file is
/// JSON alone.
fn container<'f>(model: Model, file: &'f [u8]) -> Result<(&'f [u8], Option<&'f [u8]>)> {
    if word(file, 0) != Some(GLB_MAGIC) {
        return Ok((file, None));
    }
    let (Some(version), Some(length)) = (word(file, 4), word(file, 8)) else {
        return Err(model.bad("is a GLB too short for its header"));
    };
    match version {
        2 => {}
        1 => {
            return Err(model.unsupported(
                "is a GLB of version 1, glTF 1's binary container, where the engine reads glTF 2",
            ));
        }
        _ => {
            return Err(model.bad(format!(
                "is a GLB of version {version}, which glTF doesn't have"
            )));
        }
    }
    let file = (file.get(..length as usize))
        .filter(|_| length >= 12)
        .ok_or_else(|| {
            model.bad(format!(
                "is a GLB whose header says it's {length} bytes, but it's {}",
                file.len()
            ))
        })?;
    let (mut json, mut bin) = (None, None);
    let (mut at, mut chunk) = (12, 0);
    while at < file.len() {
        let past_end = || model.bad("has a GLB chunk past its end");
        let (Some(length), Some(kind)) = (word(file, at), word(file, at + 4)) else {
            return Err(past_end());
        };
        let start = at + 8;
        let end = (start.checked_add(length as usize))
            .filter(|&end| end <= file.len())
            .ok_or_else(past_end)?;
        match (chunk, kind) {
            (0, JSON_CHUNK) => json = Some(&file[start..end]),
            (0, _) => return Err(model.bad("is a GLB that doesn't start with a JSON chunk")),
            (1, BIN_CHUNK) => bin = Some(&file[start..end]),
            // A chunk of a type glTF leaves to extensions, which it says to skip.
            _ => {}
        }
        (at, chunk) = (end, chunk + 1);
    }
    let json = json.ok_or_else(|| model.bad("is a GLB without a JSON chunk"))?;
    Ok((json, bin))
}

/// The little-endian `u32` at `at` in `bytes`, if they reach that far.
fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let bytes = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// Fails unless `asset` says the file is glTF 2.x, and needs no later version than 2.0.
fn check_version(model: Model, asset: &json::Asset) -> Result<()> {
    let not_a_version = |field: &str, version: &str| {
        model.bad(format!(
            "has asset.{field} {version:?}, which isn't a glTF version"
        ))
    };
    let version = &asset.version;
    // Only the major number, since glTF 1's versions can have more parts, such as "1.0.1".
    match version.split('.').next().map(str::parse::<u32>) {
        Some(Ok(2)) => {}
        Some(Ok(_)) => {
            return Err(
                model.unsupported(format!("is glTF {version}, where the engine reads glTF 2"))
            );
        }
        _ => return Err(not_a_version("version", version)),
    }
    if parse_version(version).is_none() {
        return Err(not_a_version("version", version));
    }
    if let Some(min) = &asset.min_version {
        let needs = parse_version(min).ok_or_else(|| not_a_version("minVersion", min))?;
        if needs > (2, 0) {
            return Err(model.unsupported(format!(
                "needs glTF {min} at least, where the engine reads glTF 2.0"
            )));
        }
    }
    Ok(())
}

/// A glTF version's major and minor numbers, such as (2, 0) for "2.0".
fn parse_version(version: &str) -> Option<(u32, u32)> {
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// A model as it's read: its file's JSON, and what's been read of it, each once.
struct Reader<'a, F> {
    model: Model<'a>,
    read_file: F,
    gltf: &'a json::Gltf,
    /// The GLB's BIN chunk, if it has one.
    bin: Option<&'a [u8]>,
    /// Each buffer read, by its index: exactly its byteLength.
    buffers: BTreeMap<u32, Cow<'a, [u8]>>,
    /// Each accessor read as an attribute, by its index: its elements' components, in order, and
    /// whether they're all finite; which a primitive copies.
    floats: BTreeMap<u32, (Vec<f32>, bool)>,
    /// Each accessor read as indices, by its index, which a primitive copies.
    indices: BTreeMap<u32, Vec<u32>>,
    /// Each mesh read, by its index: where the primitives it draws are in `imported.primitives`.
    meshes: BTreeMap<u32, Range<usize>>,
    /// Each image read, by its index: where it is in `imported.images`.
    images: BTreeMap<u32, usize>,
    /// What each image in `imported.images` holds, as the first texture that samples it says:
    /// colors, `None`, or a map of data.
    holds: BTreeMap<usize, Option<Map>>,
    /// By node: the part it is, if the scene reaches it.
    parts_of: Vec<Option<usize>>,
    imported: Imported,
    memory: Memory<'a>,
}

#[cfg(test)]
pub(crate) mod tests;
