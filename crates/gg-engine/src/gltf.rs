//! glTF 2.0 models: reads a model's `.gltf` or `.glb` file, and the buffers and images it names,
//! into what its default scene draws, for the engine to add as entities, meshes and textures.
//! Reading adds nothing anywhere, so a model that fails to read leaves no trace.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::io;
use std::mem;
use std::ops::Range;
use std::path::Path;

use gg_core::math::{Mat4, Quat, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{AlphaMode, Wrap};

use crate::Transform;
use crate::image::{self, Image};

/// What `read` gives: the default scene's parts, the primitives they draw and the images those
/// sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Imported {
    /// The scene's nodes, and a child for each primitive of a mesh with more than one, in
    /// pre-order: each part after its parent.
    pub(crate) parts: Vec<Part>,
    /// Each primitive the scene draws, once, in the order the walk first reaches it.
    pub(crate) primitives: Vec<Primitive>,
    /// Each image a reached material samples, decoded, once, in the order first reached.
    pub(crate) images: Vec<Image>,
    /// What the file has that the engine ignores, for an info event, such as "1 animation" or
    /// "extension KHR_texture_transform".
    pub(crate) ignored: Vec<String>,
}

/// A node of the scene, or a primitive of a node's mesh: what an entity holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Part {
    /// Where it is in its parent's space, or in the model's if it has none.
    pub(crate) transform: Transform,
    /// Its parent's index in `parts`, which comes before its own.
    pub(crate) parent: Option<usize>,
    /// The index in `primitives` of what it draws, if it draws anything.
    pub(crate) primitive: Option<usize>,
}

/// Triangles, with what their vertices and their material give them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Primitive {
    /// The mesh's name, or its index, then the primitive's index, such as "Cesium_Milk_Truck/2".
    pub(crate) name: String,
    pub(crate) positions: Vec<[f32; 3]>,
    /// As many as the positions: (0, 0) where the material samples no texture.
    pub(crate) uvs: Vec<[f32; 2]>,
    /// As many as the positions, linear RGBA: white without COLOR_0, alpha 1 for a VEC3.
    pub(crate) colors: Vec<[f32; 4]>,
    /// Three for each triangle; 0..count where the primitive has none.
    pub(crate) indices: Vec<u32>,
    pub(crate) material: Material,
}

/// How a primitive looks: its base color, which the material's factor and texture and the
/// primitive's vertex colors multiply together, and how its alpha covers what's behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Material {
    /// The base color factor, linear RGBA.
    pub(crate) color: [f32; 4],
    /// The index in `images` of the base color texture's image, whose colors are sRGB-encoded.
    pub(crate) image: Option<usize>,
    /// Whether the texture magnifies to the nearest texel, rather than blending the nearest four.
    pub(crate) nearest: bool,
    /// How the texture wraps, the same across and down.
    pub(crate) wrap: Wrap,
    pub(crate) alpha: AlphaMode,
    /// Whether the back of each triangle shows, as well as its front, where its vertices go
    /// counterclockwise.
    pub(crate) double_sided: bool,
}

impl Default for Material {
    /// glTF's default material, for a primitive without one: white, untextured, opaque and
    /// single-sided.
    fn default() -> Self {
        Self {
            color: [1.0; 4],
            image: None,
            nearest: false,
            wrap: Wrap::Repeat,
            alpha: AlphaMode::Opaque,
            double_sided: false,
        }
    }
}

/// The most memory reading a model may take, in bytes, for what it makes of its files: buffers read
/// from files and data URIs, images decoded, primitives' vertices, indices and names, and parts.
/// Past what a model that fits a GPU needs, as four textures 8,192 pixels square and ten million
/// vertices take about 1.5 GiB, so that a small file can't fill memory by naming its data many
/// times, such as one PNG's bytes as many images.
pub(crate) const MAX_MEMORY: u64 = 4 << 30;

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

    const UNLIT: &str = "KHR_materials_unlit";
    if let Some(extension) = gltf.extensions_required.iter().find(|&e| e != UNLIT) {
        return Err(model.unsupported(format!(
            "requires extension {extension}, which the engine doesn't read"
        )));
    }
    let mut ignored = Vec::new();
    for (count, what) in [
        (gltf.animations.len(), "animation"),
        (gltf.cameras.len(), "camera"),
    ] {
        match count {
            0 => {}
            1 => ignored.push(format!("1 {what}")),
            _ => ignored.push(format!("{count} {what}s")),
        }
    }
    ignored.extend(
        (gltf.extensions_used.iter())
            .filter(|&e| e != UNLIT)
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
        imported: Imported {
            ignored,
            ..Imported::default()
        },
        held: 0,
        memory,
    };
    reader.scene()?;
    Ok(reader.imported)
}

const GLB_MAGIC: u32 = u32::from_le_bytes(*b"glTF");
const JSON_CHUNK: u32 = u32::from_le_bytes(*b"JSON");
const BIN_CHUNK: u32 = u32::from_le_bytes(*b"BIN\0");

const UNSIGNED_BYTE: u32 = 5121;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const FLOAT: u32 = 5126;
const TRIANGLES: u32 = 4;
const NEAREST: u32 = 9728;
const CLAMP_TO_EDGE: u32 = 33071;
const MIRRORED_REPEAT: u32 = 33648;
const REPEAT: u32 = 10497;

/// How far a matrix may be from the one its translation, rotation and scale compose back to, as a
/// fraction of each column's length: past what rounding gives, as of a matrix printed to four
/// places, and short of a shear that would show.
const DECOMPOSES_WITHIN: f32 = 1e-3;

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
    /// Each accessor read as an attribute, by its index: its elements' components, in order.
    floats: BTreeMap<u32, Vec<f32>>,
    /// Each accessor read as indices, by its index.
    indices: BTreeMap<u32, Vec<u32>>,
    /// Each mesh read, by its index: where the primitives it draws are in `imported.primitives`.
    meshes: BTreeMap<u32, Range<usize>>,
    /// Each image read, by its index: where it is in `imported.images`.
    images: BTreeMap<u32, usize>,
    imported: Imported,
    /// The bytes held of what [`MAX_MEMORY`] counts, and the most that may be.
    held: u64,
    memory: u64,
}

impl<'a, F: Fn(&Path, u64) -> io::Result<Vec<u8>>> Reader<'a, F> {
    /// Counts `bytes` more held, failing once they're past the memory reading may take. Counts
    /// follow what they count, so reading takes at most an image, a primitive or a data URI more.
    fn hold(&mut self, bytes: usize) -> Result<()> {
        self.held = self.held.saturating_add(bytes as u64);
        if self.held > self.memory {
            return Err(self.model.unsupported(format!(
                "takes more than {} MiB to read, of its buffers, images, vertices and nodes",
                self.memory >> 20
            )));
        }
        Ok(())
    }

    /// Reads the default scene into parts, from its roots down, each before its children.
    fn scene(&mut self) -> Result<()> {
        let (model, gltf) = (self.model, self.gltf);
        let index = (gltf.scene)
            .or((!gltf.scenes.is_empty()).then_some(0))
            .ok_or_else(|| {
                model.unsupported("has no scene: it's a library of assets rather than a model")
            })?;
        let scene = model.item(&gltf.scenes, "scene", index)?;
        let mut reached = vec![false; gltf.nodes.len()];
        // A stack of its own rather than recursion, so that no chain of nodes is too deep.
        let mut stack: Vec<(u32, Option<usize>)> =
            scene.nodes.iter().rev().map(|&node| (node, None)).collect();
        while let Some((index, parent)) = stack.pop() {
            let node = model.item(&gltf.nodes, "node", index)?;
            if mem::replace(&mut reached[index as usize], true) {
                return Err(model.bad(format!(
                    "reaches node {index} twice: it's in a cycle, or has two parents"
                )));
            }
            if node.skin.is_some() {
                return Err(model.unsupported(format!(
                    "has node {index} skinned, which the engine doesn't draw"
                )));
            }
            // A node's morph weights replace its mesh's, which are only their default.
            let weights = match (node.mesh, node.weights.is_empty()) {
                (Some(mesh), true) => &model.item(&gltf.meshes, "mesh", mesh)?.weights,
                _ => &node.weights,
            };
            if weights.iter().any(|&weight| weight != 0.0) {
                return Err(model.unsupported(format!(
                    "has node {index} with morph weights other than 0, which the engine doesn't \
                     draw"
                )));
            }
            let transform = self.transform(index, node)?;
            let primitives = match node.mesh {
                Some(mesh) => self.mesh(mesh)?,
                None => 0..0,
            };
            let part = self.imported.parts.len();
            self.imported.parts.push(Part {
                transform,
                parent,
                primitive: (primitives.len() == 1).then_some(primitives.start),
            });
            if primitives.len() > 1 {
                // An entity draws one primitive, so each of the mesh's is a child of the node's.
                (self.imported.parts).extend(primitives.map(|primitive| Part {
                    transform: Transform::IDENTITY,
                    parent: Some(part),
                    primitive: Some(primitive),
                }));
            }
            self.hold((self.imported.parts.len() - part) * size_of::<Part>())?;
            stack.extend(node.children.iter().rev().map(|&child| (child, Some(part))));
        }
        Ok(())
    }

    /// Node `index`'s transform: its matrix decomposed, or its translation, rotation and scale.
    fn transform(&self, index: u32, node: &json::Node) -> Result<Transform> {
        let model = self.model;
        let Some(matrix) = node.matrix else {
            let rotation = match node.rotation {
                Some(rotation) => unit(Quat::from_array(rotation)).ok_or_else(|| {
                    model.bad(format!(
                        "has node {index} whose rotation is 0, or too large to make unit"
                    ))
                })?,
                None => Quat::IDENTITY,
            };
            return Ok(Transform {
                translation: node.translation.map_or(Vec3::ZERO, Vec3::from_array),
                rotation,
                scale: node.scale.map_or(Vec3::ONE, Vec3::from_array),
            });
        };
        if node.translation.is_some() || node.rotation.is_some() || node.scale.is_some() {
            return Err(model.bad(format!(
                "has node {index} with a matrix and a translation, rotation or scale too"
            )));
        }
        let doesnt_decompose = || {
            model.bad(format!(
                "has node {index} whose matrix doesn't decompose into translation, rotation and \
                 scale"
            ))
        };
        let matrix = Mat4::from_cols_array(&matrix);
        if matrix.row(3) != Vec4::W {
            return Err(doesnt_decompose());
        }
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        let rotation = unit(rotation).ok_or_else(doesnt_decompose)?;
        if !scale.is_finite() {
            return Err(doesnt_decompose());
        }
        // A shear, or a column of 0, composes back to another matrix. Each column is compared with
        // its own length, so that a small scale on one axis hides no shear along it.
        let composed = Mat4::from_scale_rotation_translation(scale, rotation, translation);
        let decomposes = (0..3).all(|i| {
            let (column, back) = (matrix.col(i), composed.col(i));
            let within = Vec4::splat(DECOMPOSES_WITHIN * column.length());
            (back - column).abs().cmple(within).all()
        });
        if !decomposes {
            return Err(doesnt_decompose());
        }
        Ok(Transform {
            translation,
            rotation,
            scale,
        })
    }

    /// Reads mesh `index`, once: where the primitives it draws are in `imported.primitives`.
    fn mesh(&mut self, index: u32) -> Result<Range<usize>> {
        if let Some(primitives) = self.meshes.get(&index) {
            return Ok(primitives.clone());
        }
        let (model, gltf) = (self.model, self.gltf);
        let mesh = model.item(&gltf.meshes, "mesh", index)?;
        let start = self.imported.primitives.len();
        for (i, primitive) in mesh.primitives.iter().enumerate() {
            if let Some(primitive) = self.primitive(index, mesh, i, primitive)? {
                self.imported.primitives.push(primitive);
            }
        }
        let primitives = start..self.imported.primitives.len();
        self.meshes.insert(index, primitives.clone());
        Ok(primitives)
    }

    /// Reads primitive `index` of mesh `mesh`, which is mesh `mesh_index`: `None` if it has no
    /// positions, which glTF says not to draw.
    fn primitive(
        &mut self,
        mesh_index: u32,
        mesh: &json::Mesh,
        index: usize,
        primitive: &json::Primitive,
    ) -> Result<Option<Primitive>> {
        let model = self.model;
        let what = format!("primitive {index} of mesh {mesh_index}");
        let Some(&position) = primitive.attributes.get("POSITION") else {
            return Ok(None);
        };
        match primitive.mode.unwrap_or(TRIANGLES) {
            TRIANGLES => {}
            mode => {
                let topology = match mode {
                    0 => "points",
                    1 => "lines",
                    2 => "a line loop",
                    3 => "a line strip",
                    5 => "a triangle strip",
                    6 => "a triangle fan",
                    _ => {
                        return Err(
                            model.bad(format!("{what} has mode {mode}, which glTF doesn't have"))
                        );
                    }
                };
                return Err(model.unsupported(format!(
                    "{what} draws {topology}, where the engine draws triangle lists alone"
                )));
            }
        }
        let (material, tex_coord) = match primitive.material {
            Some(material) => self.material(material)?,
            None => (Material::default(), None),
        };
        let (_, positions) = self.attribute(&what, "POSITION", position, &[3], false)?;
        let positions = positions.as_chunks::<3>().0.to_vec();
        let vertices = positions.len();
        let count = u32::try_from(vertices).map_err(|_| {
            model.bad(format!(
                "{what} has more vertices than 32-bit indices reach"
            ))
        })?;
        let indices = match primitive.indices {
            Some(indices) => self.indices(&what, indices, count)?,
            None if vertices.is_multiple_of(3) => (0..count).collect(),
            None => {
                return Err(model.bad(format!(
                    "{what} has {vertices} vertices and no indices, which isn't whole triangles"
                )));
            }
        };
        let same_count = |semantic: &str, elements: usize| {
            if elements == vertices {
                Ok(())
            } else {
                Err(model.bad(format!(
                    "{what}'s {semantic} has {elements} elements, where its POSITION has {vertices}"
                )))
            }
        };
        let uvs = match tex_coord {
            Some(set) => {
                let semantic = format!("TEXCOORD_{set}");
                let &accessor = primitive.attributes.get(&semantic).ok_or_else(|| {
                    model.bad(format!(
                        "{what} has no {semantic}, which its material's texture reads"
                    ))
                })?;
                let (_, uvs) = self.attribute(&what, &semantic, accessor, &[2], true)?;
                let uvs = uvs.as_chunks::<2>().0.to_vec();
                same_count(&semantic, uvs.len())?;
                uvs
            }
            None => vec![[0.0; 2]; vertices],
        };
        let colors = match primitive.attributes.get("COLOR_0") {
            Some(&accessor) => {
                let (components, colors) =
                    self.attribute(&what, "COLOR_0", accessor, &[3, 4], true)?;
                let colors: Vec<[f32; 4]> = if components == 3 {
                    let rgb = colors.as_chunks::<3>().0.iter();
                    rgb.map(|&[r, g, b]| [r, g, b, 1.0]).collect()
                } else {
                    colors.as_chunks::<4>().0.to_vec()
                };
                same_count("COLOR_0", colors.len())?;
                colors
            }
            None => vec![[1.0; 4]; vertices],
        };
        let name = match &mesh.name {
            Some(name) => format!("{name}/{index}"),
            None => format!("{mesh_index}/{index}"),
        };
        // Its own, as primitives that share accessors or a mesh's name each have a copy.
        let vertex = size_of::<[f32; 3]>() + size_of::<[f32; 2]>() + size_of::<[f32; 4]>();
        self.hold(vertices * vertex + size_of_val(indices.as_slice()) + name.len())?;
        Ok(Some(Primitive {
            name,
            positions,
            uvs,
            colors,
            indices,
            material,
        }))
    }

    /// Material `index`, and the TEXCOORD set its base color texture reads, if it has one.
    fn material(&mut self, index: u32) -> Result<(Material, Option<u32>)> {
        let (model, gltf) = (self.model, self.gltf);
        let material = model.item(&gltf.materials, "material", index)?;
        let alpha = match material.alpha_mode.as_deref() {
            None | Some("OPAQUE") => AlphaMode::Opaque,
            Some("MASK") => {
                let cutoff = material.alpha_cutoff.unwrap_or(0.5);
                if cutoff < 0.0 {
                    return Err(model.bad(format!(
                        "material {index}'s alphaCutoff is {cutoff}, below 0"
                    )));
                }
                AlphaMode::Mask { cutoff }
            }
            Some("BLEND") => AlphaMode::Blend,
            Some(mode) => {
                return Err(model.bad(format!(
                    "material {index}'s alphaMode {mode:?} isn't glTF's"
                )));
            }
        };
        let pbr = &material.pbr_metallic_roughness;
        let mut read = Material {
            color: pbr.base_color_factor.unwrap_or([1.0; 4]),
            alpha,
            double_sided: material.double_sided,
            ..Material::default()
        };
        let Some(info) = &pbr.base_color_texture else {
            return Ok((read, None));
        };
        let texture = model.item(&gltf.textures, "texture", info.index)?;
        if let Some(sampler_index) = texture.sampler {
            let sampler = model.item(&gltf.samplers, "sampler", sampler_index)?;
            let wrap = |mode: Option<u32>| match mode.unwrap_or(REPEAT) {
                CLAMP_TO_EDGE => Ok(Wrap::Clamp),
                MIRRORED_REPEAT => Ok(Wrap::Mirror),
                REPEAT => Ok(Wrap::Repeat),
                mode => Err(model.bad(format!(
                    "sampler {sampler_index} has wrap mode {mode}, which glTF doesn't have"
                ))),
            };
            let (across, down) = (wrap(sampler.wrap_s)?, wrap(sampler.wrap_t)?);
            if across != down {
                return Err(model.unsupported(format!(
                    "sampler {sampler_index} wraps S and T differently, which the engine doesn't \
                     draw"
                )));
            }
            read.wrap = across;
            read.nearest = sampler.mag_filter == Some(NEAREST);
        }
        let source = texture.source.ok_or_else(|| {
            model.unsupported(format!(
                "texture {} has no source, the image the engine samples",
                info.index
            ))
        })?;
        read.image = Some(self.image(source)?);
        Ok((read, Some(info.tex_coord)))
    }

    /// Image `index`, decoded once: where it is in `imported.images`.
    fn image(&mut self, index: u32) -> Result<usize> {
        if let Some(&at) = self.images.get(&index) {
            return Ok(at);
        }
        let (model, gltf) = (self.model, self.gltf);
        let image = model.item(&gltf.images, "image", index)?;
        let bytes = match (&image.uri, image.buffer_view) {
            // A byte past the most an image's file holds, so that a longer one fails to decode.
            (Some(uri), None) => {
                let most = image::MAX_FILE + 1;
                Cow::Owned(self.uri(&format!("image {index}"), uri, most)?)
            }
            (None, Some(view)) => Cow::Borrowed(self.view(view)?.1),
            _ => {
                return Err(model.bad(format!(
                    "image {index} has both a uri and a bufferView, or neither"
                )));
            }
        };
        let decoded = image::decode(&bytes).map_err(|problem| {
            model.bad(format!(
                "image {index} doesn't decode as a PNG or a JPEG: {problem}"
            ))
        })?;
        // Decoded for each image, as images that share a source each decode it.
        self.hold(decoded.pixels.len())?;
        let at = self.imported.images.len();
        self.imported.images.push(decoded);
        self.images.insert(index, at);
        Ok(at)
    }

    /// Accessor `index`, which `what` reads as attribute `semantic`, once: how many components
    /// each element has, and all of them in order. glTF allows it if each element has a number of
    /// components in `components`, and they're floats, or, if `normalized`, normalized unsigned
    /// bytes or shorts.
    fn attribute(
        &mut self,
        what: &str,
        semantic: &str,
        index: u32,
        components: &[usize],
        normalized: bool,
    ) -> Result<(usize, &[f32])> {
        let (model, gltf) = (self.model, self.gltf);
        let accessor = model.item(&gltf.accessors, "accessor", index)?;
        let component = match (accessor.component_type, accessor.normalized) {
            (FLOAT, false) => Some(Component::Float),
            (UNSIGNED_BYTE, true) if normalized => Some(Component::Byte),
            (UNSIGNED_SHORT, true) if normalized => Some(Component::Short),
            _ => None,
        };
        let count = match accessor.kind.as_str() {
            "SCALAR" => Some(1),
            "VEC2" => Some(2),
            "VEC3" => Some(3),
            "VEC4" => Some(4),
            _ => None,
        };
        let (Some(component), Some(count)) = (component, count.filter(|c| components.contains(c)))
        else {
            return Err(model.bad(format!(
                "{what} reads {semantic} from accessor {index}, {}, which glTF doesn't allow for it",
                accessor.describe()
            )));
        };
        if !self.floats.contains_key(&index) {
            let size = component.size();
            let (bytes, stride) = self.accessor_bytes(index, accessor, count * size)?;
            let elements = bytes.chunks(stride).map(|element| &element[..count * size]);
            let floats: Vec<f32> = match component {
                Component::Float => (elements.flat_map(|element| element.as_chunks::<4>().0))
                    .map(|&float| f32::from_le_bytes(float))
                    .collect(),
                Component::Byte => (elements.flatten())
                    .map(|&byte| f32::from(byte) / 255.0)
                    .collect(),
                Component::Short => (elements.flat_map(|element| element.as_chunks::<2>().0))
                    .map(|&short| f32::from(u16::from_le_bytes(short)) / 65535.0)
                    .collect(),
            };
            if !floats.iter().all(|float| float.is_finite()) {
                return Err(model.bad(format!("accessor {index} holds a number that isn't finite")));
            }
            self.floats.insert(index, floats);
        }
        Ok((count, &self.floats[&index]))
    }

    /// Accessor `index`, which `what` reads as its indices, once, checked to be whole triangles,
    /// each of `vertices` vertices.
    fn indices(&mut self, what: &str, index: u32, vertices: u32) -> Result<Vec<u32>> {
        let (model, gltf) = (self.model, self.gltf);
        let accessor = model.item(&gltf.accessors, "accessor", index)?;
        let size = match (
            accessor.kind.as_str(),
            accessor.component_type,
            accessor.normalized,
        ) {
            ("SCALAR", UNSIGNED_BYTE, false) => 1,
            ("SCALAR", UNSIGNED_SHORT, false) => 2,
            ("SCALAR", UNSIGNED_INT, false) => 4,
            _ => {
                return Err(model.bad(format!(
                    "{what} reads indices from accessor {index}, {}, which glTF doesn't allow \
                     for them",
                    accessor.describe()
                )));
            }
        };
        if !accessor.count.is_multiple_of(3) {
            return Err(model.bad(format!(
                "{what} has {} indices, which isn't whole triangles",
                accessor.count
            )));
        }
        if !self.indices.contains_key(&index) {
            let (bytes, stride) = self.accessor_bytes(index, accessor, size)?;
            let indices = (bytes.chunks(stride))
                .map(|element| match element[..size] {
                    [byte] => u32::from(byte),
                    [a, b] => u32::from(u16::from_le_bytes([a, b])),
                    [a, b, c, d] => u32::from_le_bytes([a, b, c, d]),
                    _ => unreachable!("indices are 1, 2 or 4 bytes"),
                })
                .collect();
            self.indices.insert(index, indices);
        }
        let indices = &self.indices[&index];
        if let Some((at, &past)) = indices.iter().enumerate().find(|&(_, &i)| i >= vertices) {
            return Err(model.bad(format!(
                "{what} has index {past} at {at}, past its {vertices} vertices"
            )));
        }
        Ok(indices.clone())
    }

    /// The bytes of accessor `index`'s elements, of `size` bytes each, from the first's start to the
    /// last's end, and the stride from each to the next, once they're checked to be within its
    /// buffer view, and that within its buffer.
    fn accessor_bytes(
        &mut self,
        index: u32,
        accessor: &json::Accessor,
        size: usize,
    ) -> Result<(&[u8], usize)> {
        let model = self.model;
        if accessor.sparse.is_some() {
            return Err(model.unsupported(format!(
                "accessor {index} is sparse, which the engine doesn't read"
            )));
        }
        let view_index = accessor.buffer_view.ok_or_else(|| {
            model.unsupported(format!(
                "accessor {index} has no buffer view, which the engine doesn't read"
            ))
        })?;
        if accessor.count == 0 {
            return Err(model.bad(format!("accessor {index} has no elements")));
        }
        let (view, bytes) = self.view(view_index)?;
        let size = size as u64;
        let stride = view.byte_stride.unwrap_or(size);
        if stride < size {
            return Err(model.bad(format!(
                "accessor {index}'s elements are {size} bytes, more than buffer view \
                 {view_index}'s stride of {stride}"
            )));
        }
        // Checked before anything is allocated for the elements, so that a count far past the
        // bytes there are fails rather than allocating for it.
        let end = (accessor.count - 1)
            .checked_mul(stride)
            .and_then(|start| start.checked_add(size))
            .and_then(|length| length.checked_add(accessor.byte_offset))
            .filter(|&end| end <= bytes.len() as u64)
            .ok_or_else(|| {
                model.bad(format!(
                    "accessor {index} reads past buffer view {view_index}"
                ))
            })?;
        Ok((
            &bytes[accessor.byte_offset as usize..end as usize],
            stride as usize,
        ))
    }

    /// Buffer view `index`, and its bytes, once they're checked to be within its buffer.
    fn view(&mut self, index: u32) -> Result<(&'a json::BufferView, &[u8])> {
        let (model, gltf) = (self.model, self.gltf);
        let view = model.item(&gltf.buffer_views, "buffer view", index)?;
        let buffer = self.buffer(view.buffer)?;
        let range = (view.byte_offset.checked_add(view.byte_length))
            .filter(|&end| end <= buffer.len() as u64)
            .map(|end| view.byte_offset as usize..end as usize)
            .ok_or_else(|| {
                model.bad(format!(
                    "buffer view {index} reads past buffer {}",
                    view.buffer
                ))
            })?;
        Ok((view, &buffer[range]))
    }

    /// Buffer `index`'s bytes, read once: exactly its byteLength.
    fn buffer(&mut self, index: u32) -> Result<&[u8]> {
        if !self.buffers.contains_key(&index) {
            let (model, gltf) = (self.model, self.gltf);
            let buffer = model.item(&gltf.buffers, "buffer", index)?;
            let bytes = match (&buffer.uri, self.bin) {
                // No further into a file than its byteLength, nor than a byte past the memory
                // reading may still take.
                (Some(uri), _) => {
                    let room = self.memory.saturating_sub(self.held).saturating_add(1);
                    let most = buffer.byte_length.min(room);
                    Cow::Owned(self.uri(&format!("buffer {index}"), uri, most)?)
                }
                (None, Some(bin)) if index == 0 => Cow::Borrowed(bin),
                (None, _) => {
                    return Err(model.bad(format!(
                        "buffer {index} has no uri, and isn't a GLB's BIN chunk"
                    )));
                }
            };
            // Read for each buffer, as buffers that share a file each read it.
            if let Cow::Owned(bytes) = &bytes {
                self.hold(bytes.len())?;
            }
            let length = (usize::try_from(buffer.byte_length).ok())
                .filter(|&length| length <= bytes.len())
                .ok_or_else(|| {
                    model.bad(format!(
                        "buffer {index} is {} bytes, short of its byteLength of {}",
                        bytes.len(),
                        buffer.byte_length
                    ))
                })?;
            let bytes = match bytes {
                Cow::Borrowed(bytes) => Cow::Borrowed(&bytes[..length]),
                Cow::Owned(mut bytes) => {
                    bytes.truncate(length);
                    Cow::Owned(bytes)
                }
            };
            self.buffers.insert(index, bytes);
        }
        Ok(&self.buffers[&index])
    }

    /// The bytes that `uri`, `what`'s, names: a data URI's, or those of a file in the model's
    /// folder, as far as `most` bytes.
    fn uri(&self, what: &str, uri: &str, most: u64) -> Result<Vec<u8>> {
        let model = self.model;
        if uri
            .get(..5)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
        {
            let (header, data) = uri.split_once(',').unwrap_or((uri, ""));
            if !header.ends_with(";base64") {
                return Err(model.bad(format!("{what}'s data URI isn't base64")));
            }
            return base64(data)
                .ok_or_else(|| model.bad(format!("{what}'s data URI isn't valid base64")));
        }
        let path = percent_decode(uri).ok_or_else(|| {
            model.bad(format!(
                "{what}'s URI {uri:?} has a % that isn't followed by two hex digits, or isn't \
                 UTF-8"
            ))
        })?;
        // Relative paths alone, as glTF has them: no scheme, drive or root, and no backslash,
        // which Windows would take for a separator.
        if path.starts_with('/') || path.contains(['\\', ':']) {
            return Err(model.bad(format!("{what}'s URI {uri:?} isn't a relative path")));
        }
        // And none up a folder, so that a model reads its own folder's files alone.
        if path.split('/').any(|segment| segment == "..") {
            return Err(model.bad(format!(
                "{what}'s URI {uri:?} has a `..`, where a model reads files in its folder alone"
            )));
        }
        let file = model.path.parent().unwrap_or(Path::new("")).join(path);
        (self.read_file)(&file, most).map_err(|error| {
            model
                .bad(format!("{what}'s file {} can't be read", file.display()))
                .with_source(error)
        })
    }
}

/// `rotation` scaled to unit length; `None` if it's 0, or too large to scale.
fn unit(rotation: Quat) -> Option<Quat> {
    let length = rotation.length();
    (length > 0.0 && length.is_finite()).then(|| rotation / length)
}

/// The type of an attribute's components that glTF allows: floats, or unsigned bytes or shorts
/// normalized to 0 to 1.
#[derive(Clone, Copy)]
enum Component {
    Float,
    Byte,
    Short,
}

impl Component {
    fn size(self) -> usize {
        match self {
            Self::Float => 4,
            Self::Byte => 1,
            Self::Short => 2,
        }
    }
}

/// Decodes base64 as RFC 4648 has it, with its padding, as data URIs hold it: `None` for a
/// character outside its alphabet, or padding that's missing or out of place.
pub(crate) fn base64(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let quads = text.as_chunks::<4>().0;
    let mut bytes = Vec::with_capacity(quads.len() * 3);
    for (i, quad) in quads.iter().enumerate() {
        let padding = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if padding > 2 || (padding > 0 && i + 1 < quads.len()) {
            return None;
        }
        let mut bits = 0;
        for &c in &quad[..4 - padding] {
            let sextet = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return None,
            };
            bits = bits << 6 | u32::from(sextet);
        }
        let [_, decoded @ ..] = (bits << (6 * padding)).to_be_bytes();
        bytes.extend_from_slice(&decoded[..3 - padding]);
    }
    Some(bytes)
}

/// Decodes a URI's percent escapes: `None` for a `%` not followed by two hex digits, or for what
/// isn't UTF-8.
fn percent_decode(uri: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(uri.len());
    let mut rest = uri.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        if byte == b'%' {
            let hex = |at: usize| char::from(*after.get(at)?).to_digit(16);
            bytes.push((hex(0)? * 16 + hex(1)?) as u8);
            rest = &after[2..];
        } else {
            bytes.push(byte);
            rest = after;
        }
    }
    String::from_utf8(bytes).ok()
}

/// The parts of a glTF file's JSON that reading uses, which serde reads with glTF's defaults,
/// skipping the rest, extras and extensions among them. Every list is in the file's order, and
/// attributes are by name, so nothing follows a hash's order.
mod json {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde::de::IgnoredAny;

    /// The only field that a file of any glTF version is sure to have of the same type.
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Versioned {
        pub(super) asset: Asset,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Asset {
        pub(super) version: String,
        pub(super) min_version: Option<String>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Gltf {
        pub(super) extensions_used: Vec<String>,
        pub(super) extensions_required: Vec<String>,
        pub(super) scene: Option<u32>,
        pub(super) scenes: Vec<Scene>,
        pub(super) nodes: Vec<Node>,
        pub(super) meshes: Vec<Mesh>,
        pub(super) accessors: Vec<Accessor>,
        pub(super) buffer_views: Vec<BufferView>,
        pub(super) buffers: Vec<Buffer>,
        pub(super) materials: Vec<Material>,
        pub(super) textures: Vec<Texture>,
        pub(super) samplers: Vec<Sampler>,
        pub(super) images: Vec<Image>,
        pub(super) animations: Vec<IgnoredAny>,
        pub(super) cameras: Vec<IgnoredAny>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Scene {
        pub(super) nodes: Vec<u32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Node {
        pub(super) children: Vec<u32>,
        pub(super) mesh: Option<u32>,
        pub(super) skin: Option<u32>,
        /// Column-major.
        pub(super) matrix: Option<[f32; 16]>,
        pub(super) translation: Option<[f32; 3]>,
        /// XYZW.
        pub(super) rotation: Option<[f32; 4]>,
        pub(super) scale: Option<[f32; 3]>,
        pub(super) weights: Vec<f32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Mesh {
        pub(super) name: Option<String>,
        pub(super) primitives: Vec<Primitive>,
        pub(super) weights: Vec<f32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Primitive {
        pub(super) attributes: BTreeMap<String, u32>,
        pub(super) indices: Option<u32>,
        pub(super) material: Option<u32>,
        /// Triangles if it's `None`.
        pub(super) mode: Option<u32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Accessor {
        pub(super) buffer_view: Option<u32>,
        pub(super) byte_offset: u64,
        pub(super) component_type: u32,
        pub(super) normalized: bool,
        pub(super) count: u64,
        #[serde(rename = "type")]
        pub(super) kind: String,
        pub(super) sparse: Option<IgnoredAny>,
    }

    impl Accessor {
        /// What its elements are, such as "of VEC3s of component type 5126", for errors.
        pub(super) fn describe(&self) -> String {
            let normalized = if self.normalized { ", normalized" } else { "" };
            format!(
                "of {}s of component type {}{normalized}",
                self.kind, self.component_type
            )
        }
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct BufferView {
        pub(super) buffer: u32,
        pub(super) byte_offset: u64,
        pub(super) byte_length: u64,
        pub(super) byte_stride: Option<u64>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Buffer {
        pub(super) uri: Option<String>,
        pub(super) byte_length: u64,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Material {
        pub(super) pbr_metallic_roughness: Pbr,
        pub(super) alpha_mode: Option<String>,
        pub(super) alpha_cutoff: Option<f32>,
        pub(super) double_sided: bool,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Pbr {
        pub(super) base_color_factor: Option<[f32; 4]>,
        pub(super) base_color_texture: Option<TextureInfo>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct TextureInfo {
        pub(super) index: u32,
        pub(super) tex_coord: u32,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Texture {
        pub(super) sampler: Option<u32>,
        pub(super) source: Option<u32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Sampler {
        pub(super) mag_filter: Option<u32>,
        pub(super) wrap_s: Option<u32>,
        pub(super) wrap_t: Option<u32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Image {
        pub(super) uri: Option<String>,
        pub(super) buffer_view: Option<u32>,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::Cell;

    use serde_json::{Value, json};

    use super::*;

    /// A BIN chunk as tests build it, and its buffer views, each starting at a multiple of 4 bytes.
    #[derive(Default)]
    struct Bin {
        bytes: Vec<u8>,
        views: Vec<Value>,
    }

    impl Bin {
        /// Adds `bytes` as a buffer view of buffer 0, its elements `stride` apart if that's given,
        /// and returns its index.
        fn view(&mut self, bytes: &[u8], stride: Option<u32>) -> usize {
            while !self.bytes.len().is_multiple_of(4) {
                self.bytes.push(0);
            }
            let mut view = json!({
                "buffer": 0,
                "byteOffset": self.bytes.len(),
                "byteLength": bytes.len(),
            });
            if let Some(stride) = stride {
                view["byteStride"] = json!(stride);
            }
            self.bytes.extend_from_slice(bytes);
            self.views.push(view);
            self.views.len() - 1
        }
    }

    /// `values` as glTF stores floats.
    fn floats(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    const PIXELS: [u8; 8] = [10, 20, 30, 255, 40, 50, 60, 128];

    /// A PNG of `PIXELS`, two pixels in a row.
    fn png() -> Vec<u8> {
        image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 2, &PIXELS, None)
    }

    /// `bytes` in base64, padded.
    fn encode(bytes: &[u8]) -> String {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut text = String::new();
        for chunk in bytes.chunks(3) {
            let bits = (chunk.iter().enumerate())
                .fold(0, |bits, (i, &byte)| bits | u32::from(byte) << (16 - 8 * i));
            for i in 0..4 {
                let sextet = (bits >> (18 - 6 * i) & 63) as usize;
                text.push(if i <= chunk.len() {
                    char::from(alphabet[sextet])
                } else {
                    '='
                });
            }
        }
        text
    }

    /// A GLB of `json`, and of `bin` as its BIN chunk unless it's empty, each padded as glTF pads
    /// them.
    fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let pad = |bytes: &[u8], with: u8| {
            let mut padded = bytes.to_vec();
            while !padded.len().is_multiple_of(4) {
                padded.push(with);
            }
            padded
        };
        let mut chunks = vec![(JSON_CHUNK, pad(json.as_bytes(), b' '))];
        if !bin.is_empty() {
            chunks.push((BIN_CHUNK, pad(bin, 0)));
        }
        let length = 12 + chunks.iter().map(|(_, data)| 8 + data.len()).sum::<usize>();
        let mut file = Vec::new();
        for word in [GLB_MAGIC, 2, length as u32] {
            file.extend(word.to_le_bytes());
        }
        for (kind, data) in chunks {
            file.extend((data.len() as u32).to_le_bytes());
            file.extend(kind.to_le_bytes());
            file.extend(data);
        }
        file
    }

    /// Reads `file` as model "m" at `path` in folder "models", with `files` beside it there, each
    /// as far as it's asked, as `dirs::read_file` reads.
    fn read_files(path: &str, file: &[u8], files: &[(&str, &[u8])]) -> Result<Imported> {
        let folder = Path::new("models");
        let read_file = |asked: &Path, most: u64| {
            let bytes = if asked == folder.join(path) {
                file
            } else {
                (files.iter().find(|(name, _)| folder.join(name) == asked))
                    .map(|&(_, bytes)| bytes)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?
            };
            Ok(bytes[..bytes.len().min(usize::try_from(most).unwrap_or(usize::MAX))].to_vec())
        };
        read("m", &folder.join(path), read_file, MAX_MEMORY)
    }

    fn read_glb(file: &[u8]) -> Result<Imported> {
        read_files("m.glb", file, &[])
    }

    /// A GLB of a model of each kind of node, primitive and material that reading takes.
    pub(crate) fn model() -> Vec<u8> {
        let mut bin = Bin::default();
        // Three vertices' positions and texture coordinates, interleaved.
        let interleaved = bin.view(
            &floats(&[0., 0., 0., 0., 0., 1., 0., 0., 1., 0., 0., 1., 0., 0., 1.]),
            Some(20),
        );
        let bytes = bin.view(&[0, 1, 2], None);
        let shorts = bin.view(&[2, 0, 1, 0, 0, 0], None);
        // Normalized RGB, with a byte after each, as glTF aligns vertices' attributes.
        let colors = bin.view(&[255, 0, 0, 7, 0, 255, 0, 7, 0, 0, 255, 7], Some(4));
        let positions = bin.view(&floats(&[0., 0., 1., 1., 0., 1., 0., 1., 1.]), None);
        // Normalized 16-bit RGBA, little-endian: 1, 0 and 0x8000 / 0xffff.
        let wide = bin.view(
            &[
                0xff, 0xff, 0, 0, 0, 0x80, 0xff, 0xff, // the first vertex
                0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, // the second
                0, 0x80, 0, 0x80, 0, 0x80, 0, 0, // the third
            ],
            None,
        );
        let image = bin.view(&png(), None);
        let gltf = json!({
            "asset": {"version": "2.0", "generator": "a test"},
            "extensionsUsed": ["KHR_materials_unlit", "KHR_texture_transform"],
            "extensionsRequired": ["KHR_materials_unlit"],
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [
                // Mirrored and moved, by a matrix.
                {
                    "matrix": [-2, 0, 0, 0, 0, 3, 0, 0, 0, 0, 4, 0, 1, 2, 3, 1],
                    "mesh": 0,
                    "children": [1, 2],
                },
                // A rotation of length 2, which is the identity once it's unit, and morph weights
                // of 0, which replace its mesh's.
                {"translation": [0, 5, 0], "rotation": [0, 0, 0, 2], "mesh": 1, "weights": [0]},
                // The first mesh again.
                {"translation": [7, 0, 0], "mesh": 0},
            ],
            "meshes": [
                {"name": "pair", "primitives": [
                    {"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "indices": 2, "material": 0},
                    {
                        "attributes": {"POSITION": 0, "TEXCOORD_0": 1, "COLOR_0": 4},
                        "indices": 3,
                        "material": 1,
                        "mode": 4,
                    },
                ]},
                // A primitive without positions, which isn't drawn, then one without indices or
                // a material, with 16-bit colors.
                {"weights": [0.5], "primitives": [
                    {"attributes": {}},
                    {"attributes": {"POSITION": 5, "COLOR_0": 6}},
                ]},
            ],
            "accessors": [
                {"bufferView": interleaved, "componentType": 5126, "count": 3, "type": "VEC3"},
                {
                    "bufferView": interleaved,
                    "byteOffset": 12,
                    "componentType": 5126,
                    "count": 3,
                    "type": "VEC2",
                },
                {"bufferView": bytes, "componentType": 5121, "count": 3, "type": "SCALAR"},
                {"bufferView": shorts, "componentType": 5123, "count": 3, "type": "SCALAR"},
                {
                    "bufferView": colors,
                    "componentType": 5121,
                    "normalized": true,
                    "count": 3,
                    "type": "VEC3",
                },
                {"bufferView": positions, "componentType": 5126, "count": 3, "type": "VEC3"},
                {
                    "bufferView": wide,
                    "componentType": 5123,
                    "normalized": true,
                    "count": 3,
                    "type": "VEC4",
                },
            ],
            "materials": [
                {
                    "pbrMetallicRoughness": {
                        "baseColorFactor": [1, 0.5, 0.25, 1],
                        "baseColorTexture": {"index": 0},
                    },
                    "alphaMode": "MASK",
                    "alphaCutoff": 0.25,
                },
                {
                    "pbrMetallicRoughness": {"baseColorTexture": {"index": 1, "texCoord": 0}},
                    "alphaMode": "BLEND",
                    "doubleSided": true,
                },
            ],
            // Two textures of one image.
            "textures": [{"source": 0, "sampler": 0}, {"source": 0}],
            "samplers": [{"magFilter": 9728, "minFilter": 9986, "wrapS": 33071, "wrapT": 33071}],
            // Said to be a JPEG, but a PNG, as its bytes say.
            "images": [{"bufferView": image, "mimeType": "image/jpeg"}],
            "buffers": [{"byteLength": bin.bytes.len()}],
            "bufferViews": bin.views,
            "animations": [{"channels": [], "samplers": []}],
            "cameras": [{"type": "perspective"}, {"type": "orthographic"}],
        });
        glb(&gltf.to_string(), &bin.bytes)
    }

    #[test]
    fn a_model_reads_into_parts_primitives_and_images() {
        let imported = read_glb(&model()).expect("a model");
        let part = |transform, parent, primitive| Part {
            transform,
            parent,
            primitive,
        };
        let moved = |x, y, z| Transform {
            translation: Vec3::new(x, y, z),
            ..Transform::IDENTITY
        };
        let mirrored = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::new(-2.0, 3.0, 4.0),
        };
        let identity = Transform::IDENTITY;
        assert_eq!(
            imported.parts,
            [
                // The first node, with each of its mesh's two primitives as a child.
                part(mirrored, None, None),
                part(identity, Some(0), Some(0)),
                part(identity, Some(0), Some(1)),
                part(moved(0.0, 5.0, 0.0), Some(0), Some(2)),
                // The same primitives again, read once.
                part(moved(7.0, 0.0, 0.0), Some(0), None),
                part(identity, Some(4), Some(0)),
                part(identity, Some(4), Some(1)),
            ]
        );
        let triangle = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let white = vec![[1.0; 4]; 3];
        let half = 32768.0 / 65535.0;
        assert_eq!(
            imported.primitives,
            [
                Primitive {
                    name: "pair/0".to_owned(),
                    positions: triangle.clone(),
                    uvs: uvs.clone(),
                    colors: white,
                    indices: vec![0, 1, 2],
                    material: Material {
                        color: [1.0, 0.5, 0.25, 1.0],
                        image: Some(0),
                        nearest: true,
                        wrap: Wrap::Clamp,
                        alpha: AlphaMode::Mask { cutoff: 0.25 },
                        double_sided: false,
                    },
                },
                Primitive {
                    name: "pair/1".to_owned(),
                    positions: triangle,
                    uvs,
                    colors: vec![
                        [1.0, 0.0, 0.0, 1.0],
                        [0.0, 1.0, 0.0, 1.0],
                        [0.0, 0.0, 1.0, 1.0]
                    ],
                    indices: vec![2, 1, 0],
                    material: Material {
                        image: Some(0),
                        alpha: AlphaMode::Blend,
                        double_sided: true,
                        ..Material::default()
                    },
                },
                Primitive {
                    name: "1/1".to_owned(),
                    positions: vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]],
                    uvs: vec![[0.0; 2]; 3],
                    colors: vec![
                        [1.0, 0.0, half, 1.0],
                        [0.0, 1.0, 0.0, 1.0],
                        [half, half, half, 0.0]
                    ],
                    indices: vec![0, 1, 2],
                    material: Material::default(),
                },
            ]
        );
        let image = Image {
            size: [2, 1],
            pixels: PIXELS.to_vec(),
        };
        assert_eq!(imported.images, [image]);
        assert_eq!(
            imported.ignored,
            [
                "1 animation",
                "2 cameras",
                "extension KHR_texture_transform"
            ]
        );
    }

    #[test]
    fn a_model_reads_the_same_every_time() {
        let model = model();
        let read = || read_glb(&model).expect("a model");
        assert_eq!(read(), read());
    }

    #[test]
    fn a_model_cut_short_or_with_a_byte_changed_fails_without_panicking() {
        let model = model();
        for length in 0..model.len() {
            assert!(read_glb(&model[..length]).is_err(), "{length} bytes");
        }
        // Some changes, such as to padding, leave a model that reads.
        for at in 0..model.len() {
            let mut changed = model.clone();
            changed[at] = 0xff;
            let _ = read_glb(&changed);
        }
    }

    #[test]
    fn a_gltf_reads_buffers_and_images_from_data_uris_and_files_beside_it() {
        let positions_and_uvs =
            floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 1.]);
        // A file a byte longer than its buffer.
        let indices: Vec<u8> = ([0u32, 2, 1].iter().flat_map(|index| index.to_le_bytes()))
            .chain([9])
            .collect();
        let gltf = json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"name": "triangle", "primitives": [{
                "attributes": {"POSITION": 0, "TEXCOORD_0": 1},
                "indices": 2,
                "material": 0,
            }]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {
                    "bufferView": 0,
                    "byteOffset": 36,
                    "componentType": 5126,
                    "count": 3,
                    "type": "VEC2",
                },
                {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"},
            ],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
            "textures": [{"source": 0}],
            "images": [{"uri": "textures/a%20b.png"}],
            "buffers": [
                {
                    "byteLength": 60,
                    "uri": format!(
                        "data:application/octet-stream;base64,{}",
                        encode(&positions_and_uvs)
                    ),
                },
                {"byteLength": 12, "uri": "my%20data.bin"},
            ],
            "bufferViews": [{"buffer": 0, "byteLength": 60}, {"buffer": 1, "byteLength": 12}],
        });
        let png = png();
        let files: [(&str, &[u8]); 2] = [("my data.bin", &indices), ("textures/a b.png", &png)];
        let imported = read_files("m.gltf", gltf.to_string().as_bytes(), &files).expect("a model");
        let part = Part {
            transform: Transform::IDENTITY,
            parent: None,
            primitive: Some(0),
        };
        assert_eq!(imported.parts, [part]);
        let primitive = Primitive {
            name: "triangle/0".to_owned(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            colors: vec![[1.0; 4]; 3],
            indices: vec![0, 2, 1],
            material: Material {
                image: Some(0),
                ..Material::default()
            },
        };
        assert_eq!(imported.primitives, [primitive]);
        assert_eq!(imported.images[0].pixels, PIXELS);
    }

    #[test]
    fn a_glb_that_doesnt_hold_together_is_refused() {
        let json = r#"{"asset": {"version": "2.0"}, "scenes": [{"nodes": []}]}"#;
        let whole = glb(json, &[]);
        assert_eq!(
            read_glb(&whole).expect("an empty scene"),
            Imported::default()
        );
        let with = |at: usize, word: u32| {
            let mut file = whole.clone();
            file[at..at + 4].copy_from_slice(&word.to_le_bytes());
            file
        };
        let (unsupported, bad) = ("engine.unsupported_model", "engine.bad_model");
        let cases = [
            ("too short", whole[..10].to_vec(), bad, "too short"),
            (
                "another magic",
                with(0, u32::from_le_bytes(*b"glTX")),
                bad,
                "isn't glTF's JSON",
            ),
            ("version 1", with(4, 1), unsupported, "version 1"),
            ("version 3", with(4, 3), bad, "version 3"),
            (
                "longer than its bytes",
                with(8, whole.len() as u32 + 4),
                bad,
                "header says",
            ),
            ("a chunk past its end", with(12, 1000), bad, "past its end"),
            ("no chunks", with(8, 12), bad, "without a JSON chunk"),
            (
                "a BIN chunk first",
                with(16, BIN_CHUNK),
                bad,
                "doesn't start with a JSON chunk",
            ),
        ];
        for (case, file, code, problem) in cases {
            let error = read_glb(&file).expect_err(case);
            assert_eq!(error.code(), code, "{case}: {error}");
            assert!(error.to_string().contains(problem), "{case}: {error}");
        }
    }

    #[test]
    fn base64_decodes_as_rfc_4648_has_it() {
        let vectors = [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ];
        for (text, bytes) in vectors {
            assert_eq!(base64(text).as_deref(), Some(bytes.as_bytes()), "{text}");
        }
        let every_byte: Vec<u8> = (0..=255).collect();
        for length in [254, 255, 256] {
            let bytes = &every_byte[..length];
            assert_eq!(base64(&encode(bytes)).as_deref(), Some(bytes));
        }
        // Characters outside the alphabet, such as URL-safe base64's, and padding missing or out
        // of place.
        for text in [
            "Zm9v!A==",
            "Zm9v YmFy",
            "Zm9v-_==",
            "Zg",
            "Zg=",
            "Z===",
            "Zg==Zg==",
            "Zm=v",
        ] {
            assert_eq!(base64(text), None, "{text}");
        }
    }

    #[test]
    fn uris_percent_decode() {
        let decoded = [
            ("my%20data.bin", "my data.bin"),
            ("grande_sph%C3%A8re.png", "grande_sph\u{e8}re.png"),
            ("grande_sph\u{e8}re.png", "grande_sph\u{e8}re.png"),
        ];
        for (uri, path) in decoded {
            assert_eq!(percent_decode(uri).as_deref(), Some(path), "{uri}");
        }
        for uri in ["%zz", "a%2", "a%", "%+f", "%FF"] {
            assert_eq!(percent_decode(uri), None, "{uri}");
        }
    }

    /// The JSON and BIN chunk of a model of one textured triangle, for tests to break.
    fn triangle() -> (Value, Vec<u8>) {
        let mut bin = Bin::default();
        let positions = bin.view(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]), None);
        let uvs = bin.view(&floats(&[0., 0., 1., 0., 0., 1.]), None);
        // A fourth index, past the vertices, for a test to read.
        let indices = bin.view(&[0, 0, 1, 0, 2, 0, 3, 0], None);
        let image = bin.view(&png(), None);
        // View 4: NaNs, for a test to read as positions.
        bin.view(&[0xff; 12], None);
        let gltf = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{
                "attributes": {"POSITION": 0, "TEXCOORD_0": 1},
                "indices": 2,
                "material": 0,
            }]}],
            "accessors": [
                {"bufferView": positions, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": uvs, "componentType": 5126, "count": 3, "type": "VEC2"},
                {"bufferView": indices, "componentType": 5123, "count": 3, "type": "SCALAR"},
            ],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
            "textures": [{"source": 0, "sampler": 0}],
            "samplers": [{}],
            "images": [{"bufferView": image, "mimeType": "image/png"}],
            "buffers": [{"byteLength": bin.bytes.len()}],
            "bufferViews": bin.views,
        });
        (gltf, bin.bytes)
    }

    /// Removes `key` from the object `value`.
    fn remove(value: &mut Value, key: &str) {
        value.as_object_mut().expect("an object").remove(key);
    }

    #[test]
    fn a_matrix_rounded_as_it_was_printed_still_decomposes() {
        let (mut gltf, bin) = triangle();
        // A turn of 30 degrees about Z, to four places.
        let matrix = [
            0.866, 0.5, 0.0, 0.0, -0.5, 0.866, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        gltf["nodes"][0]["matrix"] = json!(matrix);
        let imported = read_glb(&glb(&gltf.to_string(), &bin)).expect("a model");
        let rotation = imported.parts[0].transform.rotation;
        let turn = Quat::from_rotation_z(gg_core::math::to_radians(30.0));
        assert!(rotation.abs_diff_eq(turn, 1e-4), "{rotation}");
    }

    /// The code of the error that a change to a model's JSON makes, what its message says, and the
    /// change.
    type Refusal = (&'static str, &'static str, fn(&mut Value));

    #[test]
    fn what_the_engine_doesnt_draw_or_that_isnt_gltf_is_refused_with_its_code() {
        let (gltf, bin) = triangle();
        read_glb(&glb(&gltf.to_string(), &bin)).expect("a triangle");
        let (unsupported, bad) = ("engine.unsupported_model", "engine.bad_model");
        let cases: &[Refusal] = &[
            // What the engine doesn't draw.
            (unsupported, "glTF 1.0", |g| {
                g["asset"]["version"] = json!("1.0")
            }),
            (unsupported, "glTF 2.1 at least", |g| {
                g["asset"]["minVersion"] = json!("2.1");
            }),
            (unsupported, "no scene", |g| {
                remove(g, "scene");
                remove(g, "scenes");
            }),
            (unsupported, "triangle lists", |g| {
                g["meshes"][0]["primitives"][0]["mode"] = json!(5);
            }),
            (unsupported, "sparse", |g| {
                g["accessors"][0]["sparse"] = json!({"count": 1});
            }),
            (unsupported, "no buffer view", |g| {
                remove(&mut g["accessors"][0], "bufferView");
            }),
            (unsupported, "skinned", |g| g["nodes"][0]["skin"] = json!(0)),
            (unsupported, "morph weights", |g| {
                g["meshes"][0]["weights"] = json!([0.5]);
            }),
            (unsupported, "morph weights", |g| {
                g["nodes"][0]["weights"] = json!([0, 1]);
            }),
            (unsupported, "wraps S and T differently", |g| {
                g["samplers"][0]["wrapS"] = json!(33071);
            }),
            (unsupported, "no source", |g| {
                remove(&mut g["textures"][0], "source")
            }),
            (unsupported, "KHR_draco_mesh_compression", |g| {
                g["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
            }),
            // What isn't glTF.
            (bad, "isn't glTF's JSON", |g| {
                g["nodes"] = json!({"root": {}})
            }),
            (bad, "glTF version", |g| {
                g["asset"]["version"] = json!("two")
            }),
            (bad, "glTF version", |g| remove(g, "asset")),
            (bad, "has no scene 1", |g| g["scene"] = json!(1)),
            (bad, "has no node 5", |g| {
                g["scenes"][0]["nodes"] = json!([5])
            }),
            (bad, "has no mesh 1", |g| g["nodes"][0]["mesh"] = json!(1)),
            (bad, "node 0 twice", |g| {
                g["nodes"][0]["children"] = json!([0])
            }),
            (bad, "node 0 twice", |g| {
                g["scenes"][0]["nodes"] = json!([1, 2]);
                let nodes = g["nodes"].as_array_mut().expect("nodes");
                nodes.extend([json!({"children": [0]}), json!({"children": [0]})]);
            }),
            (bad, "a matrix and", |g| {
                g["nodes"][0]["matrix"] = json!([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
                g["nodes"][0]["scale"] = json!([1, 1, 1]);
            }),
            // A shear, a projection, and a column of 0.
            (bad, "doesn't decompose", |g| {
                g["nodes"][0]["matrix"] = json!([1, 0, 0, 0, 0.5, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
            }),
            (bad, "doesn't decompose", |g| {
                g["nodes"][0]["matrix"] = json!([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, -1, 0, 0, 0, 1]);
            }),
            (bad, "doesn't decompose", |g| {
                g["nodes"][0]["matrix"] = json!([0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
            }),
            (bad, "rotation is 0", |g| {
                g["nodes"][0]["rotation"] = json!([0, 0, 0, 0])
            }),
            (bad, "mode 9", |g| {
                g["meshes"][0]["primitives"][0]["mode"] = json!(9)
            }),
            (bad, "reads POSITION from accessor 0", |g| {
                g["accessors"][0]["type"] = json!("VEC2");
            }),
            (bad, "reads POSITION from accessor 0", |g| {
                g["accessors"][0]["componentType"] = json!(5121);
                g["accessors"][0]["normalized"] = json!(true);
            }),
            (bad, "reads TEXCOORD_0 from accessor 1", |g| {
                g["accessors"][1]["componentType"] = json!(5122);
            }),
            (bad, "no TEXCOORD_0", |g| {
                let attributes = &mut g["meshes"][0]["primitives"][0]["attributes"];
                remove(attributes, "TEXCOORD_0");
            }),
            (bad, "no TEXCOORD_1", |g| {
                let texture = &mut g["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"];
                texture["texCoord"] = json!(1);
            }),
            (bad, "TEXCOORD_0 has 2 elements", |g| {
                g["accessors"][1]["count"] = json!(2)
            }),
            (bad, "reads indices from accessor 2", |g| {
                g["accessors"][2]["componentType"] = json!(5126);
            }),
            (bad, "past its 3 vertices", |g| {
                g["accessors"][2]["byteOffset"] = json!(2)
            }),
            (bad, "isn't whole triangles", |g| {
                g["accessors"][2]["count"] = json!(4)
            }),
            (bad, "isn't whole triangles", |g| {
                remove(&mut g["meshes"][0]["primitives"][0], "indices");
                g["accessors"][0]["count"] = json!(2);
            }),
            (bad, "isn't finite", |g| {
                g["accessors"][0]["bufferView"] = json!(4);
                g["accessors"][0]["count"] = json!(1);
            }),
            (bad, "has no elements", |g| {
                g["accessors"][0]["count"] = json!(0)
            }),
            // A byte short of the accessor.
            (bad, "reads past buffer view 0", |g| {
                g["bufferViews"][0]["byteLength"] = json!(35);
            }),
            // A count far past the bytes there are, which fails before anything is allocated for
            // it.
            (bad, "reads past buffer view 0", |g| {
                g["accessors"][0]["count"] = json!(4_000_000_000u64);
            }),
            (bad, "elements are 12 bytes", |g| {
                g["bufferViews"][0]["byteStride"] = json!(8)
            }),
            (bad, "reads past buffer 0", |g| {
                g["bufferViews"][0]["byteOffset"] = json!(1_000_000);
            }),
            (bad, "short of its byteLength", |g| {
                g["buffers"][0]["byteLength"] = json!(1_000_000);
            }),
            (bad, "has no buffer 1", |g| {
                g["bufferViews"][0]["buffer"] = json!(1)
            }),
            (bad, "alphaMode", |g| {
                g["materials"][0]["alphaMode"] = json!("CUTOUT")
            }),
            (bad, "alphaCutoff", |g| {
                g["materials"][0]["alphaMode"] = json!("MASK");
                g["materials"][0]["alphaCutoff"] = json!(-1);
            }),
            (bad, "wrap mode 1234", |g| {
                g["samplers"][0] = json!({"wrapS": 1234, "wrapT": 1234});
            }),
            (bad, "both a uri and a bufferView", |g| {
                g["images"][0]["uri"] = json!("a.png");
            }),
            (bad, "doesn't decode as a PNG or a JPEG", |g| {
                g["images"][0]["bufferView"] = json!(0);
            }),
            (bad, "isn't base64", |g| {
                g["buffers"][0]["uri"] = json!("data:application/octet-stream,AAAA");
            }),
            (bad, "isn't valid base64", |g| {
                g["buffers"][0]["uri"] = json!("data:application/octet-stream;base64,AAA");
            }),
            (bad, "two hex digits", |g| {
                g["buffers"][0]["uri"] = json!("m%zz.bin")
            }),
            (bad, "can't be read", |g| {
                g["buffers"][0]["uri"] = json!("missing.bin")
            }),
            // URIs with a scheme, a root, a drive or a backslash, as they are or percent-encoded.
            (bad, "isn't a relative path", |g| {
                g["buffers"][0]["uri"] = json!("https://example.com/m.bin");
            }),
            (bad, "isn't a relative path", |g| {
                g["buffers"][0]["uri"] = json!("/m.bin")
            }),
            (bad, "isn't a relative path", |g| {
                g["buffers"][0]["uri"] = json!("C:/m.bin")
            }),
            (bad, "isn't a relative path", |g| {
                g["buffers"][0]["uri"] = json!("models\\m.bin");
            }),
            (bad, "isn't a relative path", |g| {
                g["buffers"][0]["uri"] = json!("%5C%5Cserver%5Cm.bin");
            }),
            (bad, "isn't a relative path", |g| {
                g["buffers"][0]["uri"] = json!("C%3A/m.bin")
            }),
            (bad, "isn't a relative path", |g| {
                g["buffers"][0]["uri"] = json!("//server/share/m.bin");
            }),
            // URIs that go up a folder, as they are or percent-encoded.
            (bad, "has a `..`", |g| {
                g["buffers"][0]["uri"] = json!("../m.bin")
            }),
            (bad, "has a `..`", |g| {
                g["buffers"][0]["uri"] = json!("textures/../../m.bin");
            }),
            (bad, "has a `..`", |g| g["buffers"][0]["uri"] = json!("..")),
            (bad, "has a `..`", |g| {
                g["buffers"][0]["uri"] = json!("%2E%2e/m.bin")
            }),
            (bad, "has a `..`", |g| {
                g["buffers"][0]["uri"] = json!("..%2Fm.bin")
            }),
            (bad, "has a `..`", |g| {
                g["images"][0] = json!({"uri": "../../../../dev/zero"});
            }),
        ];
        for (code, problem, change) in cases {
            let mut gltf = gltf.clone();
            change(&mut gltf);
            let error = read_glb(&glb(&gltf.to_string(), &bin)).expect_err(problem);
            assert_eq!(error.code(), *code, "{problem}: {error}");
            assert!(error.to_string().contains(problem), "{problem}: {error}");
        }
    }

    #[test]
    fn a_buffers_file_is_read_no_further_than_its_byte_length_or_the_memory_left() {
        // The triangle's buffer in a file of its own, with bytes past its byteLength.
        let (mut gltf, bin) = triangle();
        gltf["buffers"][0]["uri"] = json!("m.bin");
        let model = glb(&gltf.to_string(), &[]);
        let length = bin.len() as u64;
        let read_with = |file: &[u8], memory| {
            let asked = Cell::new(0);
            let read_file = |path: &Path, most: u64| {
                if path == Path::new("models/m.glb") {
                    return Ok(model.clone());
                }
                asked.set(most);
                Ok(file[..file.len().min(most as usize)].to_vec())
            };
            let read = read("m", Path::new("models/m.glb"), read_file, memory);
            (read, asked.get())
        };
        let longer = [&bin[..], &[7; 64]].concat();
        let (read, asked) = read_with(&longer, MAX_MEMORY);
        read.expect("a triangle");
        assert_eq!(asked, length);
        // A file short of its byteLength.
        let (read, _) = read_with(&bin[..bin.len() - 1], MAX_MEMORY);
        let error = read.expect_err("a short file");
        assert_eq!(error.code(), "engine.bad_model");
        let message = error.to_string();
        assert!(message.contains("short of its byteLength"), "{message}");
        // A byteLength past the memory reading may take, which reads a byte past it.
        let memory = length / 2;
        let (read, asked) = read_with(&longer, memory);
        let error = read.expect_err("a buffer past the memory");
        assert_eq!(error.code(), "engine.unsupported_model");
        assert_eq!(asked, memory + 1);
    }

    #[test]
    fn data_drawn_many_times_counts_each_time_against_the_memory_reading_may_take() {
        // A mesh of `primitives` that share `vertices`, each holding a copy, drawn by `nodes`, each
        // a part for each primitive: 30 copies of 3,000 vertices, 120 KB each; and 2,000 nodes of a
        // mesh of 8 triangles, 648 bytes of parts each.
        let model = |vertices: usize, primitives: usize, nodes: usize| {
            let mut bin = Bin::default();
            let positions = bin.view(&vec![0; vertices * 12], None);
            let primitive = json!({"attributes": {"POSITION": 0}});
            let gltf = json!({
                "asset": {"version": "2.0"},
                "scenes": [{"nodes": (0..nodes).collect::<Vec<_>>()}],
                "nodes": vec![json!({"mesh": 0}); nodes],
                "meshes": [{"primitives": vec![primitive; primitives]}],
                "accessors": [{
                    "bufferView": positions,
                    "componentType": 5126,
                    "count": vertices,
                    "type": "VEC3",
                }],
                "buffers": [{"byteLength": bin.bytes.len()}],
                "bufferViews": bin.views,
            });
            glb(&gltf.to_string(), &bin.bytes)
        };
        for (vertices, primitives, nodes) in [(3000, 30, 1), (3, 8, 2000)] {
            let model = model(vertices, primitives, nodes);
            let read_with =
                |memory| read("m", Path::new("m.glb"), |_, _| Ok(model.clone()), memory);
            read_with(MAX_MEMORY).expect("a model");
            let error = read_with(1 << 20).expect_err("a model past 1 MiB");
            assert_eq!(error.code(), "engine.unsupported_model", "{error}");
        }
    }

    #[test]
    fn images_that_share_a_source_each_count_against_the_memory_reading_may_take() {
        // Four triangles, each sampling an image of its own, all four the same PNG: 1-bit gray, 256
        // pixels square, a few hundred bytes that decode to 256 KiB of RGBA each.
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, 256, 256);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::One);
        let mut writer = encoder.write_header().expect("a header");
        let rows = [0; 256 * 256 / 8];
        writer.write_image_data(&rows).expect("the pixels");
        writer.finish().expect("a whole PNG");
        let mut bin = Bin::default();
        let positions = bin.view(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]), None);
        let uvs = bin.view(&floats(&[0., 0., 1., 0., 0., 1.]), None);
        let image = bin.view(&png, None);
        let primitive = |i| json!({"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "material": i});
        let material = |i| json!({"pbrMetallicRoughness": {"baseColorTexture": {"index": i}}});
        let gltf = json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": (0..4).map(primitive).collect::<Vec<_>>()}],
            "accessors": [
                {"bufferView": positions, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": uvs, "componentType": 5126, "count": 3, "type": "VEC2"},
            ],
            "materials": (0..4).map(material).collect::<Vec<_>>(),
            "textures": (0..4).map(|i| json!({"source": i})).collect::<Vec<_>>(),
            "images": vec![json!({"bufferView": image}); 4],
            "buffers": [{"byteLength": bin.bytes.len()}],
            "bufferViews": bin.views,
        });
        let model = glb(&gltf.to_string(), &bin.bytes);
        let read_with = |memory| read("m", Path::new("m.glb"), |_, _| Ok(model.clone()), memory);
        let imported = read_with(2 << 20).expect("four images in 2 MiB");
        assert_eq!(imported.images.len(), 4);
        let error = read_with(1 << 20).expect_err("four images past 1 MiB");
        assert_eq!(error.code(), "engine.unsupported_model");
        assert!(error.to_string().contains("more than 1 MiB"), "{error}");
    }
}
