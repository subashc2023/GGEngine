//! glTF 2.0 models: reads a model's `.gltf` or `.glb` file, and the buffers and images it names,
//! into what its default scene draws, for the engine to add as entities, meshes and textures.
//! Reading adds nothing anywhere, so a model that fails to read leaves no trace.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::io;
use std::mem;
use std::ops::Range;
use std::path::{Path, PathBuf};

use bevy_mikktspace::{Geometry, Ops, TangentSpace};
use gg_core::math::{Mat4, Quat, Vec3, Vec4, libm};
use gg_core::{Error, Result};
use gg_renderer::{AlphaMode, Map, ModelMesh, Workflow, Wrap};

use crate::animation::{Channel, Clip, Interpolation, Path as Moved, Skin, placed, skinned_box};
use crate::image::{self, Image};
use crate::package::Files;
use crate::{Light, LightKind, Transform};

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Part {
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

/// Triangles, with what their vertices and their material give them.
#[derive(Clone, Debug, PartialEq)]
pub struct Primitive {
    /// The mesh's name, or its index, then the primitive's index, such as "Cesium_Milk_Truck/2".
    pub(crate) name: String,
    pub positions: Vec<[f32; 3]>,
    /// As many as the positions: (0, 0) where the material samples no texture.
    pub uvs: Vec<[f32; 2]>,
    /// As many as the positions, linear RGBA: white without COLOR_0, alpha 1 for a VEC3.
    pub colors: Vec<[f32; 4]>,
    /// As many as the positions, unit vectors: its NORMALs, or, without them, each triangle's own,
    /// its corners apart from its neighbors', as glTF says to draw such a primitive flat; and +Z,
    /// which nothing reads, for an unlit material's without them.
    pub normals: Vec<[f32; 3]>,
    /// As many as the positions: its TANGENTs, W the bitangent's sign, or, without them, those
    /// MikkTSpace makes for a lit material with a normal map, as glTF says, its vertices apart where
    /// their corners' tangents differ; and (1, 0, 0, 1), which nothing reads, for the rest.
    pub tangents: Vec<[f32; 4]>,
    /// As many as the positions, for a primitive with JOINTS_0 and WEIGHTS_0: each vertex's four
    /// joints, of the skin that moves it; and none for the rest.
    pub joints: Vec<[u16; 4]>,
    /// As many as `joints`: how much each of a vertex's joints moves it.
    pub weights: Vec<[f32; 4]>,
    /// For a primitive a skinned node draws, the box, its least corner and its most, that its
    /// vertices reach as its skins' joints move them through the model's clips, with room to
    /// spare, which its mesh packs within and views cull it by.
    pub reach: Option<[Vec3; 2]>,
    /// Three for each triangle; 0..count where the primitive has none.
    pub indices: Vec<u32>,
    /// Its mesh as the cooker made it, its triangles optimized, with levels of detail and
    /// meshlets, and its vertices packed, as a run from a package reads it in place of the
    /// vertices and indices above, which are then empty; none as glTF is read, which a run packs
    /// as it adds the primitive.
    pub mesh: Option<ModelMesh>,
    pub(crate) material: Material,
}

impl Primitive {
    /// Whether its material blends, so that its triangles cover each other in the order they draw
    /// in, which the cooker keeps.
    pub fn blends(&self) -> bool {
        matches!(self.material.alpha, AlphaMode::Blend)
    }

    /// The bytes its vertices and indices take, as [`Memory`] counts them.
    fn held(&self) -> usize {
        size_of_val(self.positions.as_slice())
            + size_of_val(self.uvs.as_slice())
            + size_of_val(self.colors.as_slice())
            + size_of_val(self.normals.as_slice())
            + size_of_val(self.tangents.as_slice())
            + size_of_val(self.joints.as_slice())
            + size_of_val(self.weights.as_slice())
            + size_of_val(self.indices.as_slice())
    }

    /// The bytes each of its vertices' joints and weights take: none for a primitive without them.
    fn skin_bytes(&self) -> usize {
        if self.joints.is_empty() {
            0
        } else {
            size_of::<[u16; 4]>() + size_of::<[f32; 4]>()
        }
    }

    /// Gives each triangle corners of its own, facing as the triangle does, counterclockwise its
    /// front: as glTF draws a primitive without normals, flat. A triangle of no area faces +Z,
    /// covering nothing that shows. Its tangents go, which it has none of before its normals.
    ///
    /// Fails, making nothing, if `memory` hasn't room for its corners' vertices beside those it has.
    fn make_flat(&mut self, memory: &mut Memory) -> Result<()> {
        fn corners<T: Copy>(indices: &[u32], values: &[T]) -> Vec<T> {
            indices.iter().map(|&i| values[i as usize]).collect()
        }
        // A vertex a corner, but its tangent, and its index.
        let corner = VERTEX - size_of::<[f32; 4]>() + size_of::<u32>() + self.skin_bytes();
        memory.hold(self.indices.len() * corner)?;
        let before = self.held();
        let (positions, uvs, colors) = (
            corners(&self.indices, &self.positions),
            corners(&self.indices, &self.uvs),
            corners(&self.indices, &self.colors),
        );
        if !self.joints.is_empty() {
            self.joints = corners(&self.indices, &self.joints);
            self.weights = corners(&self.indices, &self.weights);
        }
        self.normals = positions
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[a, b, c]| {
                let [a, b, c] = [a, b, c].map(Vec3::from_array);
                let normal = (b - a).cross(c - a).normalize_or(Vec3::Z);
                [normal.to_array(); 3]
            })
            .collect();
        self.indices = (0..positions.len() as u32).collect();
        (self.positions, self.uvs, self.colors) = (positions, uvs, colors);
        self.tangents.clear();
        memory.release(before);
        Ok(())
    }

    /// Gives it the tangents MikkTSpace makes from its positions, normals and texture coordinates,
    /// as glTF says to for a normal map's primitive without them: a tangent for each triangle's
    /// corner, which its vertices then take, each vertex apart for each tangent of its corners,
    /// in the order its corners first reach them. A corner MikkTSpace gives none, of a triangle of
    /// no area alone, takes +X.
    ///
    /// Fails, changing nothing, if `memory` hasn't room for the tangents and the vertices apart
    /// beside those it has.
    fn make_tangents(&mut self, memory: &mut Memory) -> Result<()> {
        let count = self.indices.len();
        // MikkTSpace's tangent for each corner; then each corner's index, and the vertex and
        // tangent of each vertex apart, at most one a corner.
        let made = count * (size_of::<[f32; 4]>() + size_of::<u32>() + PLACED);
        memory.hold(made)?;
        let mut corners = Corners {
            primitive: self,
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; count],
        };
        // Its error has no values: it never fails.
        let _ = bevy_mikktspace::generate_tangents::<_, Libm>(&mut corners);
        let Corners { tangents, .. } = corners;
        let mut placed: BTreeMap<(u32, [u32; 4]), u32> = BTreeMap::new();
        let indices: Vec<u32> = (self.indices.iter().zip(&tangents))
            .map(|(&vertex, tangent)| {
                let (key, next) = ((vertex, tangent.map(f32::to_bits)), placed.len() as u32);
                *placed.entry(key).or_insert(next)
            })
            .collect();
        drop(tangents);
        let vertices = placed.len();
        memory.hold(vertices * (VERTEX + self.skin_bytes()))?;
        let skinned = if self.joints.is_empty() { 0 } else { vertices };
        let mut welded = Self {
            name: self.name.clone(),
            positions: vec![[0.0; 3]; vertices],
            uvs: vec![[0.0; 2]; vertices],
            colors: vec![[0.0; 4]; vertices],
            normals: vec![[0.0; 3]; vertices],
            tangents: vec![[0.0; 4]; vertices],
            joints: vec![[0; 4]; skinned],
            weights: vec![[0.0; 4]; skinned],
            reach: self.reach,
            indices,
            mesh: self.mesh.clone(),
            material: self.material,
        };
        for ((vertex, tangent), at) in placed {
            let (v, at) = (vertex as usize, at as usize);
            welded.positions[at] = self.positions[v];
            welded.uvs[at] = self.uvs[v];
            welded.colors[at] = self.colors[v];
            welded.normals[at] = self.normals[v];
            welded.tangents[at] = tangent.map(f32::from_bits);
            if skinned > 0 {
                welded.joints[at] = self.joints[v];
                welded.weights[at] = self.weights[v];
            }
        }
        let before = self.held();
        *self = welded;
        // Its vertices and indices before, MikkTSpace's tangents and the vertices placed, of what
        // was held, its new indices kept.
        memory.release(before + made - count * size_of::<u32>());
        Ok(())
    }
}

/// The bytes a vertex of a [`Primitive`] takes: its position, texture coordinates, color, normal
/// and tangent.
const VERTEX: usize = size_of::<[f32; 3]>() * 2 + size_of::<[f32; 2]>() + size_of::<[f32; 4]>() * 2;

/// The most bytes an entry of the `BTreeMap` that places [`Primitive::make_tangents`]' vertices
/// takes, its share of the map's nodes included: std's nodes hold 5 to 11 entries, the root fewer,
/// beside a header and an internal node's edges, which comes to about 2.5 times the entry, at most,
/// once the map holds a node's worth.
const PLACED: usize = 3 * size_of::<((u32, [u32; 4]), u32)>();

/// A primitive's triangles as MikkTSpace reads them, a face each, and the tangent it gives each
/// corner.
struct Corners<'a> {
    primitive: &'a Primitive,
    tangents: Vec<[f32; 4]>,
}

impl Corners<'_> {
    fn vertex(&self, face: usize, corner: usize) -> usize {
        self.primitive.indices[face * 3 + corner] as usize
    }
}

impl Geometry<Libm> for Corners<'_> {
    fn num_faces(&self) -> usize {
        self.primitive.indices.len() / 3
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, corner: usize) -> [f32; 3] {
        self.primitive.positions[self.vertex(face, corner)]
    }

    fn normal(&self, face: usize, corner: usize) -> [f32; 3] {
        self.primitive.normals[self.vertex(face, corner)]
    }

    fn tex_coord(&self, face: usize, corner: usize) -> [f32; 2] {
        self.primitive.uvs[self.vertex(face, corner)]
    }

    fn set_tangent(&mut self, tangent: Option<TangentSpace>, face: usize, corner: usize) {
        if let Some(tangent) = tangent {
            self.tangents[face * 3 + corner] = tangent.tangent_encoded();
        }
    }
}

/// MikkTSpace's square roots and arccosines, libm's, as the engine's are, so that a model's
/// tangents are the same on every platform, from its files or cooked.
struct Libm;

impl Ops for Libm {
    fn sqrt(x: f32) -> f32 {
        libm::sqrtf(x)
    }

    fn acos(x: f32) -> f32 {
        libm::acosf(x)
    }
}

/// How a primitive looks, as glTF's metallic-roughness materials say, or
/// KHR_materials_pbrSpecularGlossiness's: its base color, or its diffuse color, which the
/// material's factor and texture and the primitive's vertex colors multiply together; how it
/// reflects, its normal map, occlusion and emission; whether it's unlit, as KHR_materials_unlit has
/// it; and how its alpha covers what's behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Material {
    /// The base color factor, or the diffuse factor, linear RGBA.
    pub(crate) color: [f32; 4],
    /// How metallic and rough it is, or its specular color and how glossy.
    pub(crate) workflow: Workflow,
    /// The normal map's scale of its X and Y.
    pub(crate) normal_scale: f32,
    /// How much the occlusion map darkens, from 0 to 1.
    pub(crate) occlusion_strength: f32,
    /// The emissive factor, linear RGB, times KHR_materials_emissive_strength's strength.
    pub(crate) emissive: [f32; 3],
    /// Its textures, each at its [`Slot`]'s place, which it samples, if it has one.
    pub(crate) textures: [Option<Sampling>; 5],
    pub(crate) alpha: AlphaMode,
    /// Whether the back of each triangle shows, as well as its front, where its vertices go
    /// counterclockwise.
    pub(crate) double_sided: bool,
    pub(crate) unlit: bool,
}

impl Default for Material {
    /// glTF's default material, for a primitive without one: white, metallic and rough,
    /// untextured, opaque and single-sided.
    fn default() -> Self {
        Self {
            color: [1.0; 4],
            workflow: Workflow::default(),
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            textures: [None; 5],
            alpha: AlphaMode::Opaque,
            double_sided: false,
            unlit: false,
        }
    }
}

impl Material {
    /// Whether its numbers are all finite, as a model's material's must be.
    pub(crate) fn is_finite(&self) -> bool {
        let workflow = match self.workflow {
            Workflow::MetallicRoughness {
                metallic,
                roughness,
            } => metallic.is_finite() && roughness.is_finite(),
            Workflow::SpecularGlossiness {
                specular,
                glossiness,
            } => specular.is_finite() && glossiness.is_finite(),
        };
        let cutoff = match self.alpha {
            AlphaMode::Mask { cutoff } => cutoff,
            AlphaMode::Opaque | AlphaMode::Blend => 0.0,
        };
        let numbers = [self.normal_scale, self.occlusion_strength, cutoff].into_iter();
        workflow && (numbers.chain(self.color).chain(self.emissive)).all(f32::is_finite)
    }

    /// Its texture of `slot`, if it has one.
    pub(crate) fn texture(&self, slot: Slot) -> Option<Sampling> {
        self.textures[slot as usize]
    }

    /// What its texture of `slot` holds, as [`Slot::map`] says, but that a specular-glossiness
    /// map holds colors, its specular color sRGB-encoded, as KHR_materials_pbrSpecularGlossiness
    /// has it.
    pub(crate) fn map(&self, slot: Slot) -> Option<Map> {
        match (slot, self.workflow) {
            (Slot::Surface, Workflow::SpecularGlossiness { .. }) => None,
            _ => slot.map(),
        }
    }
}

/// A material's texture: the index in `images` of its image, and how it samples it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sampling {
    pub(crate) image: usize,
    /// Whether it magnifies to the nearest texel, rather than blending the nearest four.
    pub(crate) nearest: bool,
    /// How it wraps, the same across and down.
    pub(crate) wrap: Wrap,
}

/// Each of a material's textures, in the order its `textures` hold them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Base color, or diffuse.
    Color,
    /// Metallic-roughness, or specular-glossiness.
    Surface,
    Normal,
    Occlusion,
    Emissive,
}

impl Slot {
    pub(crate) const ALL: [Self; 5] = [
        Self::Color,
        Self::Surface,
        Self::Normal,
        Self::Occlusion,
        Self::Emissive,
    ];

    /// What its texture's image holds in a metallic-roughness material: colors, `None`,
    /// sRGB-encoded, or a map of data.
    pub(crate) fn map(self) -> Option<Map> {
        match self {
            Self::Color | Self::Emissive => None,
            Self::Surface | Self::Occlusion => Some(Map::Surface),
            Self::Normal => Some(Map::Normal),
        }
    }
}

/// What an image holds, as a texture samples it, in a phrase.
pub(crate) fn holds(map: Option<Map>) -> &'static str {
    match map {
        None => "colors",
        Some(Map::Surface) => "values",
        Some(Map::Normal) => "a normal map",
    }
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
        let held = self.held.saturating_add(bytes as u64);
        if held > self.most {
            return Err(self.model.unsupported(format!(
                "takes more than {} MiB to read, of its buffers, images, vertices and nodes",
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

const BYTE: u32 = 5120;
const UNSIGNED_BYTE: u32 = 5121;
const SHORT: u32 = 5122;
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

impl<'a, F: Fn(&Path, u64) -> io::Result<Vec<u8>>> Reader<'a, F> {
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
        self.parts_of = vec![None; gltf.nodes.len()];
        // Each skinned part, and its skin in glTF's order, which are read once every joint's part
        // is known.
        let mut skinned: Vec<(usize, u32)> = Vec::new();
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
            let light = match &node.extensions.lights_punctual {
                Some(light) => Some(self.light(light.light)?),
                None => None,
            };
            self.memory.hold(size_of::<Part>())?;
            let part = self.imported.parts.len();
            self.parts_of[index as usize] = Some(part);
            let one = (primitives.len() == 1).then_some(primitives.start);
            self.imported.parts.push(Part {
                transform,
                parent,
                primitive: one,
                light,
                skin: None,
            });
            if let (Some(skin), Some(_)) = (node.skin, one) {
                skinned.push((part, skin));
            }
            if primitives.len() > 1 {
                // An entity draws one primitive, so each of the mesh's is a child of the node's.
                self.memory.hold(primitives.len() * size_of::<Part>())?;
                for primitive in primitives {
                    if let Some(skin) = node.skin {
                        skinned.push((self.imported.parts.len(), skin));
                    }
                    self.imported.parts.push(Part {
                        transform: Transform::IDENTITY,
                        parent: Some(part),
                        primitive: Some(primitive),
                        light: None,
                        skin: None,
                    });
                }
            }
            stack.extend(node.children.iter().rev().map(|&child| (child, Some(part))));
        }
        // Each skin once, as it's first reached, and the parts it moves checked against it.
        let mut read: BTreeMap<u32, usize> = BTreeMap::new();
        for (part, index) in skinned {
            let skin = match read.get(&index) {
                Some(&skin) => skin,
                None => {
                    let skin = self.skin(index)?;
                    self.imported.skins.push(skin);
                    read.insert(index, self.imported.skins.len() - 1);
                    self.imported.skins.len() - 1
                }
            };
            self.imported.parts[part].skin = Some(skin);
            let primitive = self.imported.parts[part]
                .primitive
                .expect("a skinned part draws");
            let primitive = &self.imported.primitives[primitive];
            let joints = self.imported.skins[skin].joints.len();
            if primitive.joints.is_empty() {
                return Err(model.bad(format!(
                    "has primitive {}, which a node skinned by skin {index} draws, without \
                     JOINTS_0 and WEIGHTS_0",
                    primitive.name
                )));
            }
            let past = primitive
                .joints
                .iter()
                .flatten()
                .find(|&&j| usize::from(j) >= joints);
            if let Some(past) = past {
                return Err(model.bad(format!(
                    "has primitive {} moved by joint {past}, past the {joints} of skin {index}, \
                     which skins it",
                    primitive.name
                )));
            }
        }
        Ok(())
    }

    /// Skin `index`: its joints' parts, each node of them in the scene, and their inverse bind
    /// matrices, identities without them.
    fn skin(&mut self, index: u32) -> Result<Skin> {
        let (model, gltf) = (self.model, self.gltf);
        let skin = model.item(&gltf.skins, "skin", index)?;
        if skin.joints.is_empty() || skin.joints.len() > usize::from(u16::MAX) + 1 {
            return Err(model.bad(format!(
                "has skin {index} of {} joints, where glTF's skins have from 1 to 65,536",
                skin.joints.len()
            )));
        }
        self.memory
            .hold(skin.joints.len() * (size_of::<u32>() + size_of::<Mat4>()))?;
        let joints = (skin.joints.iter())
            .map(|&node| {
                let part = self.parts_of.get(node as usize).copied().flatten();
                let part = part.ok_or_else(|| {
                    model.bad(format!(
                        "has skin {index} of joint node {node}, which isn't in its scene"
                    ))
                })?;
                Ok(u32::try_from(part).expect("fewer than 2^32 parts"))
            })
            .collect::<Result<Vec<u32>>>()?;
        let inverse_binds = match skin.inverse_bind_matrices {
            Some(accessor) => {
                let what = format!("skin {index}");
                self.attribute(&what, "its inverse bind matrices", accessor, &[16], false)?;
                let matrices = self.floats[&accessor].0.as_chunks::<16>().0;
                if matrices.len() < joints.len() {
                    return Err(model.bad(format!(
                        "has skin {index} of {} joints and {} inverse bind matrices",
                        joints.len(),
                        matrices.len()
                    )));
                }
                (matrices[..joints.len()].iter())
                    .map(Mat4::from_cols_array)
                    .collect()
            }
            None => vec![Mat4::IDENTITY; joints.len()],
        };
        Ok(Skin {
            joints,
            inverse_binds,
        })
    }

    /// Reads each animation into a clip, in the file's order, its channels that move nodes the
    /// scene reaches, each checked as glTF says: its times from 0 up, each after the last, and as
    /// many values as its interpolation takes, its rotations made unit. A channel of morph weights,
    /// or of a node outside the scene, is ignored, as `imported.ignored` says.
    fn animations(&mut self) -> Result<()> {
        let (model, gltf) = (self.model, self.gltf);
        for (i, animation) in gltf.animations.iter().enumerate() {
            let name = animation.name.clone().unwrap_or_else(|| i.to_string());
            self.memory.hold(name.len())?;
            let mut channels = Vec::new();
            let mut duration: f32 = 0.0;
            for (c, channel) in animation.channels.iter().enumerate() {
                let what = format!("channel {c} of animation {i}");
                let Some(node) = channel.target.node else {
                    continue;
                };
                let path = match channel.target.path.as_str() {
                    "translation" => Moved::Translation,
                    "rotation" => Moved::Rotation,
                    "scale" => Moved::Scale,
                    "weights" => {
                        self.imported
                            .ignored
                            .push(format!("{what}, of morph weights"));
                        continue;
                    }
                    other => {
                        return Err(model.bad(format!(
                            "has {what} of path {other:?}, which glTF doesn't have"
                        )));
                    }
                };
                let part = self.parts_of.get(node as usize).copied().flatten();
                let Some(part) = part else {
                    (self.imported.ignored)
                        .push(format!("{what}, of node {node} outside the scene"));
                    continue;
                };
                let sampler = model.item(
                    &animation.samplers,
                    &format!("sampler of animation {i}"),
                    channel.sampler,
                )?;
                let interpolation = match sampler.interpolation.as_deref().unwrap_or("LINEAR") {
                    "LINEAR" => Interpolation::Linear,
                    "STEP" => Interpolation::Step,
                    "CUBICSPLINE" => Interpolation::CubicSpline,
                    other => {
                        return Err(model.bad(format!(
                            "has {what} interpolated {other:?}, which glTF doesn't have"
                        )));
                    }
                };
                self.attribute(&what, "its times", sampler.input, &[1], false)?;
                let times = self.memory.copy(&self.floats[&sampler.input].0)?;
                let ordered = times.windows(2).all(|pair| pair[0] < pair[1]);
                if !(ordered && times[0] >= 0.0) {
                    return Err(model.bad(format!(
                        "has {what} of times that don't go up from 0, each after the last"
                    )));
                }
                let rotation = path == Moved::Rotation;
                let width = if rotation { 4 } else { 3 };
                let (count, finite) = self.floats_of(
                    &what,
                    "its values",
                    sampler.output,
                    &[width],
                    rotation,
                    rotation,
                )?;
                debug_assert_eq!(count, width);
                if !finite {
                    let output = sampler.output;
                    return Err(model.bad(format!(
                        "accessor {output} holds a number that isn't finite"
                    )));
                }
                let mut values = self.memory.copy(&self.floats[&sampler.output].0)?;
                let keys = if interpolation == Interpolation::CubicSpline {
                    3
                } else {
                    1
                };
                if values.len() != times.len() * keys * width {
                    return Err(model.bad(format!(
                        "has {what} of {} times and {} values, where its interpolation takes {}",
                        times.len(),
                        values.len() / width,
                        times.len() * keys
                    )));
                }
                if rotation {
                    // Values of a cubic spline in turn its tangents and its values, of which the
                    // values alone are rotations.
                    for (v, value) in values.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        if keys == 3 && v % 3 != 1 {
                            continue;
                        }
                        let unit = unit(Quat::from_array(*value)).ok_or_else(|| {
                            model.bad(format!("has {what} of a rotation that won't make unit"))
                        })?;
                        *value = unit.to_array();
                    }
                }
                duration = duration.max(*times.last().expect("an accessor has elements"));
                channels.push(Channel {
                    part: u32::try_from(part).expect("fewer than 2^32 parts"),
                    path,
                    interpolation,
                    times,
                    values,
                });
            }
            self.memory.hold(channels.len() * size_of::<Channel>())?;
            self.imported.clips.push(Clip {
                name,
                duration,
                channels,
            });
        }
        Ok(())
    }

    /// Gives each primitive a skinned part draws its reach: the box its vertices span as the part's
    /// skin moves them, as the model rests and posed at each clip's keys' times, or at as many as
    /// [`Clip::pose_times`] gives, each clip's from the model at rest; grown by a tenth of its
    /// longest side each way, for the poses between those times, and between clips blended.
    fn reach(&mut self) -> Result<()> {
        let Imported {
            parts,
            primitives,
            skins,
            clips,
            ..
        } = &mut self.imported;
        let rest: Vec<Transform> = parts.iter().map(|part| part.transform).collect();
        let parents: Vec<Option<usize>> = parts.iter().map(|part| part.parent).collect();
        let mut poses = vec![placed(&rest, &parents)];
        for clip in clips.iter() {
            for time in clip.pose_times() {
                let mut locals = rest.clone();
                for channel in &clip.channels {
                    let local = &mut locals[channel.part as usize];
                    channel.apply(local, channel.sample(time));
                }
                poses.push(placed(&locals, &parents));
            }
        }
        for (part, skinned) in parts.iter().enumerate() {
            let (Some(skin), Some(primitive)) = (skinned.skin, skinned.primitive) else {
                continue;
            };
            let primitive = &mut primitives[primitive];
            let skin = &skins[skin];
            let positions = &primitive.positions;
            let (joints, weights) = (&primitive.joints, &primitive.weights);
            let bounds = poses
                .iter()
                .map(|pose| skinned_box(pose, part, skin, positions, joints, weights))
                .chain(primitive.reach);
            let [low, high] = bounds
                .fold([Vec3::INFINITY, Vec3::NEG_INFINITY], |[l, h], [a, b]| {
                    [l.min(a), h.max(b)]
                });
            let spare = Vec3::splat((high - low).max_element() / 10.0);
            primitive.reach = Some([low - spare, high + spare]);
        }
        Ok(())
    }

    /// Light `index` of KHR_lights_punctual's, which a node holds, checked as the extension says,
    /// of a type it has, and as [`allowed`] says. Its intensity and range are in its units, candela
    /// or lux, and meters, the engine's.
    fn light(&self, index: u32) -> Result<Light> {
        let (model, gltf) = (self.model, self.gltf);
        let lights = gltf.extensions.lights_punctual.as_ref();
        let light = model.item(lights.map_or(&[][..], |l| &l.lights), "light", index)?;
        let kind = match light.kind.as_str() {
            "directional" => LightKind::Directional,
            "point" => LightKind::Point,
            "spot" => LightKind::Spot,
            other => {
                return Err(model.bad(format!(
                    "has light {index} of type {other:?}, which KHR_lights_punctual doesn't have"
                )));
            }
        };
        let spot = light.spot.as_ref();
        let read = Light {
            kind,
            color: light.color.map_or(Vec3::ONE, Vec3::from_array),
            intensity: light.intensity.unwrap_or(1.0),
            range: light.range,
            inner_cone: spot.and_then(|spot| spot.inner_cone_angle).unwrap_or(0.0),
            outer_cone: (spot.and_then(|spot| spot.outer_cone_angle))
                .unwrap_or(std::f32::consts::FRAC_PI_4),
            // KHR_lights_punctual says nothing of shadows.
            ..Light::default()
        };
        if !allowed(&read) {
            return Err(model.bad(format!(
                "has light {index} of a negative color or intensity, a range of 0 or less, a \
                 cone's angles out of order or past a quarter turn, or a number that isn't finite"
            )));
        }
        Ok(read)
    }

    /// Node `index`'s transform: its matrix decomposed, or its translation, rotation and scale, its
    /// rotation made unit; one [`takes`] takes, as JSON's numbers are finite, and serde_json's
    /// `float_roundtrip`, which the workspace turns on, refuses one past an f32's range.
    fn transform(&self, index: u32, node: &json::Node) -> Result<Transform> {
        let model = self.model;
        let Some(matrix) = node.matrix else {
            let rotation = match node.rotation {
                Some(rotation) => unit(Quat::from_array(rotation)).ok_or_else(|| {
                    model.bad(format!(
                        "has node {index} whose rotation is 0, or too near it or too large to \
                         make unit"
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
        // Its own copies, each counted before it's made, as primitives that share accessors or a
        // mesh's name each have their own.
        self.attribute(&what, "POSITION", position, &[3], false)?;
        let positions = self.copied(position)?;
        let vertices = positions.len();
        let count = u32::try_from(vertices).map_err(|_| {
            model.bad(format!(
                "{what} has more vertices than 32-bit indices reach"
            ))
        })?;
        let indices = match primitive.indices {
            Some(indices) => {
                self.indices(&what, indices, count)?;
                self.memory.copy(&self.indices[&indices])?
            }
            None if vertices.is_multiple_of(3) => {
                self.memory.hold(vertices * size_of::<u32>())?;
                (0..count).collect()
            }
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
                self.attribute(&what, &semantic, accessor, &[2], true)?;
                let uvs = self.copied(accessor)?;
                same_count(&semantic, uvs.len())?;
                uvs
            }
            None => self.memory.filled([0.0; 2], vertices)?,
        };
        let colors = match primitive.attributes.get("COLOR_0") {
            Some(&accessor) => {
                let components = self.attribute(&what, "COLOR_0", accessor, &[3, 4], true)?;
                let colors: Vec<[f32; 4]> = if components == 3 {
                    let rgb = self.floats[&accessor].0.as_chunks::<3>().0;
                    self.memory.hold(size_of::<[f32; 4]>() * rgb.len())?;
                    rgb.iter().map(|&[r, g, b]| [r, g, b, 1.0]).collect()
                } else {
                    self.copied(accessor)?
                };
                same_count("COLOR_0", colors.len())?;
                colors
            }
            None => self.memory.filled([1.0; 4], vertices)?,
        };
        let normals = match primitive.attributes.get("NORMAL") {
            Some(&accessor) => {
                self.attribute(&what, "NORMAL", accessor, &[3], false)?;
                let normals = self.copied(accessor)?;
                same_count("NORMAL", normals.len())?;
                Some(normals)
            }
            None => None,
        };
        // Tangents that aren't all finite, as a tool can leave where a triangle's texture
        // coordinates have no area, are ignored, the primitive drawn as one without them, which
        // glTF says to make them for.
        let tangents = match primitive.attributes.get("TANGENT") {
            Some(&accessor) => {
                let (_, finite) = self.floats(&what, "TANGENT", accessor, &[4], false)?;
                let tangents = if finite {
                    Some(self.copied(accessor)?)
                } else {
                    None
                };
                if tangents.is_none() {
                    self.imported.ignored.push(format!(
                        "{what}'s TANGENT, which holds numbers that aren't finite: it's drawn as \
                         without one"
                    ));
                }
                if let Some(tangents) = &tangents {
                    same_count("TANGENT", tangents.len())?;
                }
                tangents
            }
            None => None,
        };
        let attributes = &primitive.attributes;
        let (joints, weights) = match (attributes.get("JOINTS_0"), attributes.get("WEIGHTS_0")) {
            (Some(&joints), Some(&weights)) => {
                let joints = self.joints(&what, joints)?;
                same_count("JOINTS_0", joints.len())?;
                self.attribute(&what, "WEIGHTS_0", weights, &[4], true)?;
                let weights = self.copied(weights)?;
                same_count("WEIGHTS_0", weights.len())?;
                (joints, weights)
            }
            (None, None) => (Vec::new(), Vec::new()),
            _ => {
                return Err(model.bad(format!(
                    "{what} has one of JOINTS_0 and WEIGHTS_0 without the other"
                )));
            }
        };
        if attributes.contains_key("JOINTS_1") {
            self.imported.ignored.push(format!(
                "{what}'s JOINTS_1 and WEIGHTS_1: it's moved by its first four joints"
            ));
        }
        let name = match &mesh.name {
            Some(name) => format!("{name}/{index}"),
            None => format!("{mesh_index}/{index}"),
        };
        let mut read = Primitive {
            name,
            positions,
            uvs,
            colors,
            normals: Vec::new(),
            tangents: Vec::new(),
            joints,
            weights,
            reach: None,
            indices,
            mesh: None,
            material,
        };
        // Tangents without normals are ignored, as glTF says, as the primitive is drawn flat; but
        // an unlit material reads neither, so its vertices stay as they are, welded.
        let tangents = match normals {
            Some(normals) => {
                read.normals = normals;
                tangents
            }
            None if material.unlit => {
                read.normals = self.memory.filled([0.0, 0.0, 1.0], read.positions.len())?;
                None
            }
            None => {
                read.make_flat(&mut self.memory)?;
                None
            }
        };
        match tangents {
            Some(tangents) => read.tangents = tangents,
            None if !material.unlit && material.texture(Slot::Normal).is_some() => {
                read.make_tangents(&mut self.memory)?;
            }
            None => {
                let vertices = read.positions.len();
                read.tangents = self.memory.filled([1.0, 0.0, 0.0, 1.0], vertices)?;
            }
        }
        self.memory.hold(read.name.len())?;
        Ok(Some(read))
    }

    /// Material `index`, and the TEXCOORD set its textures read, if it has any: that of the first
    /// in [`Slot`]'s order, as a primitive's vertices have one set. A material of
    /// KHR_materials_pbrSpecularGlossiness's is that, in place of its metallic-roughness, which is
    /// the extension's fallback. A texture that reads another set, or whose image another texture
    /// samples as what it doesn't, as colors where it samples a map, is left out, which
    /// `imported.ignored` says.
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
        let extensions = &material.extensions;
        let strength = (extensions.emissive_strength.as_ref())
            .and_then(|strength| strength.emissive_strength)
            .unwrap_or(1.0);
        let emissive = material.emissive_factor.unwrap_or([0.0; 3]);
        let (color, workflow, color_texture, surface_texture) =
            match &extensions.specular_glossiness {
                Some(glossy) => (
                    glossy.diffuse_factor.unwrap_or([1.0; 4]),
                    Workflow::SpecularGlossiness {
                        specular: Vec3::from_array(glossy.specular_factor.unwrap_or([1.0; 3])),
                        glossiness: glossy.glossiness_factor.unwrap_or(1.0),
                    },
                    (glossy.diffuse_texture.as_ref(), "diffuseTexture"),
                    (
                        glossy.specular_glossiness_texture.as_ref(),
                        "specularGlossinessTexture",
                    ),
                ),
                None => (
                    pbr.base_color_factor.unwrap_or([1.0; 4]),
                    Workflow::MetallicRoughness {
                        metallic: pbr.metallic_factor.unwrap_or(1.0),
                        roughness: pbr.roughness_factor.unwrap_or(1.0),
                    },
                    (pbr.base_color_texture.as_ref(), "baseColorTexture"),
                    (
                        pbr.metallic_roughness_texture.as_ref(),
                        "metallicRoughnessTexture",
                    ),
                ),
            };
        let mut read = Material {
            color,
            workflow,
            normal_scale: (material.normal_texture.as_ref())
                .and_then(|normal| normal.scale)
                .unwrap_or(1.0),
            occlusion_strength: (material.occlusion_texture.as_ref())
                .and_then(|occlusion| occlusion.strength)
                .unwrap_or(1.0),
            emissive: emissive.map(|channel| channel * strength),
            alpha,
            double_sided: material.double_sided,
            unlit: extensions.unlit.is_some(),
            ..Material::default()
        };
        // As the emissive factor times its strength can be past an f32's range, though each of the
        // file's numbers is within it.
        if !read.is_finite() {
            return Err(model.bad(format!("material {index} has a number that isn't finite")));
        }
        let infos = [
            (Slot::Color, color_texture),
            (Slot::Surface, surface_texture),
            (
                Slot::Normal,
                (
                    (material.normal_texture.as_ref()).map(|n| &n.info),
                    "normalTexture",
                ),
            ),
            (
                Slot::Occlusion,
                (
                    (material.occlusion_texture.as_ref()).map(|o| &o.info),
                    "occlusionTexture",
                ),
            ),
            (
                Slot::Emissive,
                (material.emissive_texture.as_ref(), "emissiveTexture"),
            ),
        ];
        let mut set = None;
        for (slot, (info, name)) in infos {
            let Some(info) = info else {
                continue;
            };
            let read_set = *set.get_or_insert(info.tex_coord);
            if read_set != info.tex_coord {
                self.imported.ignored.push(format!(
                    "material {index}'s {name}, which reads TEXCOORD_{} where its others read \
                     TEXCOORD_{read_set}",
                    info.tex_coord
                ));
                continue;
            }
            let held = read.map(slot);
            read.textures[slot as usize] = self.sampling(index, name, held, info.index)?;
        }
        Ok((read, set))
    }

    /// Texture `texture`, which material `material` samples as its `name`, as what `wanted` says
    /// its image holds, colors or a map: its image, its DDS image if it has one, which
    /// MSFT_texture_dds names in place of its source, and how it samples it; or `None` where
    /// another texture samples its image as what it doesn't, which `imported.ignored` says.
    fn sampling(
        &mut self,
        material: u32,
        name: &str,
        wanted: Option<Map>,
        texture: u32,
    ) -> Result<Option<Sampling>> {
        let (model, gltf) = (self.model, self.gltf);
        let index = texture;
        let texture = model.item(&gltf.textures, "texture", index)?;
        let (mut wrap, mut nearest) = (Wrap::Repeat, false);
        if let Some(sampler_index) = texture.sampler {
            let sampler = model.item(&gltf.samplers, "sampler", sampler_index)?;
            let wrapping = |mode: Option<u32>| match mode.unwrap_or(REPEAT) {
                CLAMP_TO_EDGE => Ok(Wrap::Clamp),
                MIRRORED_REPEAT => Ok(Wrap::Mirror),
                REPEAT => Ok(Wrap::Repeat),
                mode => Err(model.bad(format!(
                    "sampler {sampler_index} has wrap mode {mode}, which glTF doesn't have"
                ))),
            };
            let (across, down) = (wrapping(sampler.wrap_s)?, wrapping(sampler.wrap_t)?);
            if across != down {
                return Err(model.unsupported(format!(
                    "sampler {sampler_index} wraps S and T differently, which the engine doesn't \
                     draw"
                )));
            }
            wrap = across;
            nearest = sampler.mag_filter == Some(NEAREST);
        }
        let dds = (texture.extensions.dds.as_ref()).and_then(|dds| dds.source);
        let source = dds.or(texture.source).ok_or_else(|| {
            model.unsupported(format!(
                "texture {index} has no source, the image the engine samples"
            ))
        })?;
        let image = self.image(source)?;
        let held = *self.holds.entry(image).or_insert(wanted);
        if held != wanted {
            self.imported.ignored.push(format!(
                "material {material}'s {name}, which samples image {source} as {} where another \
                 texture samples it as {}",
                holds(wanted),
                holds(held)
            ));
            return Ok(None);
        }
        Ok(Some(Sampling {
            image,
            nearest,
            wrap,
        }))
    }

    /// Image `index`, read once, as far as its header, as [`image::read`] reads it, or as
    /// [`image::read_dds`] reads a DDS file, which it tells apart by its first bytes, for the loader
    /// to decode: where it is in `imported.images`.
    fn image(&mut self, index: u32) -> Result<usize> {
        if let Some(&at) = self.images.get(&index) {
            return Ok(at);
        }
        let (model, gltf) = (self.model, self.gltf);
        let image = model.item(&gltf.images, "image", index)?;
        // A byte past the most an image's file holds, so that a longer one fails to decode.
        let most = image::MAX_FILE + 1;
        let bytes = match (&image.uri, image.buffer_view) {
            (Some(uri), None) => self.uri(&format!("image {index}"), uri, most)?,
            (None, Some(view)) => {
                let bytes = self.view(view)?.1;
                bytes[..bytes.len().min(most as usize)].to_vec()
            }
            _ => {
                return Err(model.bad(format!(
                    "image {index} has both a uri and a bufferView, or neither"
                )));
            }
        };
        let read = if image::is_dds(&bytes) {
            image::read_dds(bytes).map_err(|problem| {
                model.bad(format!(
                    "image {index} doesn't read as a DDS of BC7: {problem}"
                ))
            })?
        } else {
            image::read(bytes).map_err(|problem| {
                model.bad(format!(
                    "image {index} doesn't decode as a PNG or a JPEG: {problem}"
                ))
            })?
        };
        // Read and decoded for each image, as images that share a source each decode it, its file
        // held until it's decoded.
        self.memory.hold(read.pixels.len() + read.decoded_len())?;
        let at = self.imported.images.len();
        self.imported.images.push(read);
        self.images.insert(index, at);
        Ok(at)
    }

    /// Accessor `index`, which `what` reads as attribute `semantic`, as [`floats`](Self::floats)
    /// reads it, failing if it holds a number that isn't finite, which glTF doesn't allow.
    fn attribute(
        &mut self,
        what: &str,
        semantic: &str,
        index: u32,
        components: &[usize],
        normalized: bool,
    ) -> Result<usize> {
        let model = self.model;
        let (count, finite) = self.floats(what, semantic, index, components, normalized)?;
        if !finite {
            return Err(model.bad(format!("accessor {index} holds a number that isn't finite")));
        }
        Ok(count)
    }

    /// Accessor `index`, which `what` reads as attribute `semantic`, once, all its elements'
    /// components in order into `floats`: how many components each element has, and whether
    /// they're all finite. glTF allows it if each element has a number of components in
    /// `components`, and they're floats, or, if `normalized`, normalized unsigned bytes or shorts.
    fn floats(
        &mut self,
        what: &str,
        semantic: &str,
        index: u32,
        components: &[usize],
        normalized: bool,
    ) -> Result<(usize, bool)> {
        self.floats_of(what, semantic, index, components, normalized, false)
    }

    /// Accessor `index`, as [`floats`](Self::floats) reads it, of normalized signed bytes or
    /// shorts too, if `signed`, each 1 at its most and -1 at its least and the one past it, as
    /// glTF allows for an animation's rotations.
    fn floats_of(
        &mut self,
        what: &str,
        semantic: &str,
        index: u32,
        components: &[usize],
        normalized: bool,
        signed: bool,
    ) -> Result<(usize, bool)> {
        let (model, gltf) = (self.model, self.gltf);
        let accessor = model.item(&gltf.accessors, "accessor", index)?;
        let component = match (accessor.component_type, accessor.normalized) {
            (FLOAT, false) => Some(Component::Float),
            (UNSIGNED_BYTE, true) if normalized => Some(Component::Byte),
            (UNSIGNED_SHORT, true) if normalized => Some(Component::Short),
            (BYTE, true) if normalized && signed => Some(Component::SignedByte),
            (SHORT, true) if normalized && signed => Some(Component::SignedShort),
            _ => None,
        };
        let count = match accessor.kind.as_str() {
            "SCALAR" => Some(1),
            "VEC2" => Some(2),
            "VEC3" => Some(3),
            "VEC4" => Some(4),
            "MAT4" => Some(16),
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
            // Checked first, so that a count past the bytes there are fails as that, then counted.
            self.accessor_bytes(index, accessor, count * size)?;
            let len = accessor.count as usize * count;
            self.memory.hold(len * size_of::<f32>())?;
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
                Component::SignedByte => (elements.flatten())
                    .map(|&byte| (f32::from(byte as i8) / 127.0).max(-1.0))
                    .collect(),
                Component::SignedShort => (elements.flat_map(|element| element.as_chunks::<2>().0))
                    .map(|&short| (f32::from(i16::from_le_bytes(short)) / 32767.0).max(-1.0))
                    .collect(),
            };
            let finite = floats.iter().all(|float| float.is_finite());
            self.floats.insert(index, (floats, finite));
        }
        Ok((count, self.floats[&index].1))
    }

    /// Accessor `index`'s elements of `N` components, which [`floats`](Self::floats) read, copied
    /// once the copy is counted.
    fn copied<const N: usize>(&mut self, index: u32) -> Result<Vec<[f32; N]>> {
        let floats = self.floats[&index].0.as_chunks::<N>().0;
        self.memory.copy(floats)
    }

    /// Accessor `index`, which `what` reads as JOINTS_0: each vertex's four joints, of unsigned
    /// bytes or shorts, as glTF allows.
    fn joints(&mut self, what: &str, index: u32) -> Result<Vec<[u16; 4]>> {
        let (model, gltf) = (self.model, self.gltf);
        let accessor = model.item(&gltf.accessors, "accessor", index)?;
        let size = match (
            accessor.kind.as_str(),
            accessor.component_type,
            accessor.normalized,
        ) {
            ("VEC4", UNSIGNED_BYTE, false) => 1,
            ("VEC4", UNSIGNED_SHORT, false) => 2,
            _ => {
                return Err(model.bad(format!(
                    "{what} reads JOINTS_0 from accessor {index}, {}, which glTF doesn't allow \
                     for it",
                    accessor.describe()
                )));
            }
        };
        // Checked first, so that a count past the bytes there are fails as that, then counted.
        self.accessor_bytes(index, accessor, 4 * size)?;
        self.memory
            .hold(accessor.count as usize * size_of::<[u16; 4]>())?;
        let (bytes, stride) = self.accessor_bytes(index, accessor, 4 * size)?;
        Ok((bytes.chunks(stride))
            .map(|element| {
                std::array::from_fn(|i| match size {
                    1 => u16::from(element[i]),
                    _ => u16::from_le_bytes([element[2 * i], element[2 * i + 1]]),
                })
            })
            .collect())
    }

    /// Accessor `index`, which `what` reads as its indices, once, into `indices`, checked to be
    /// whole triangles, each of `vertices` vertices.
    fn indices(&mut self, what: &str, index: u32, vertices: u32) -> Result<()> {
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
            // Checked first, so that a count past the bytes there are fails as that, then counted.
            self.accessor_bytes(index, accessor, size)?;
            self.memory
                .hold(accessor.count as usize * size_of::<u32>())?;
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
        Ok(())
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
                    let room =
                        (self.memory.most.saturating_sub(self.memory.held)).saturating_add(1);
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
                self.memory.hold(bytes.len())?;
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

/// `rotation` scaled to unit length; `None` if it's 0, or too near it or too large to scale to a
/// unit, as the world takes one.
fn unit(rotation: Quat) -> Option<Quat> {
    let length = rotation.length();
    let scaled = (length > 0.0 && length.is_finite()).then(|| rotation / length);
    scaled.filter(|unit| unit.is_normalized())
}

/// Whether `transform` is one a model's part may have, as the world takes it: of finite numbers,
/// its rotation a unit.
pub(crate) fn takes(transform: &Transform) -> bool {
    let Transform {
        translation,
        rotation,
        scale,
    } = transform;
    translation.is_finite() && scale.is_finite() && rotation.is_normalized()
}

/// Whether `light` is one a model's part may have, as KHR_lights_punctual allows it, of finite
/// numbers: of a color and an intensity not negative, a range more than 0 if it has one, and a
/// spot's cone's angles from 0 to the outer, which is more than the inner and at most a quarter
/// turn.
pub(crate) fn allowed(light: &Light) -> bool {
    let Light {
        color,
        intensity,
        range,
        inner_cone,
        outer_cone,
        ..
    } = *light;
    color.is_finite()
        && color.cmpge(Vec3::ZERO).all()
        && intensity.is_finite()
        && intensity >= 0.0
        && range.is_none_or(|range| range.is_finite() && range > 0.0)
        && (0.0..outer_cone).contains(&inner_cone)
        && outer_cone <= std::f32::consts::FRAC_PI_2
}

/// The type of an attribute's components that glTF allows: floats, unsigned bytes or shorts
/// normalized to 0 to 1, or, for an animation's rotations, signed ones normalized to -1 to 1.
#[derive(Clone, Copy)]
enum Component {
    Float,
    Byte,
    Short,
    SignedByte,
    SignedShort,
}

impl Component {
    fn size(self) -> usize {
        match self {
            Self::Float => 4,
            Self::Byte | Self::SignedByte => 1,
            Self::Short | Self::SignedShort => 2,
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
        pub(super) skins: Vec<Skin>,
        pub(super) animations: Vec<Animation>,
        pub(super) cameras: Vec<IgnoredAny>,
        pub(super) extensions: Extensions,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub(super) struct Extensions {
        #[serde(rename = "KHR_lights_punctual")]
        pub(super) lights_punctual: Option<LightsPunctual>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct LightsPunctual {
        pub(super) lights: Vec<Light>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Light {
        #[serde(rename = "type")]
        pub(super) kind: String,
        pub(super) color: Option<[f32; 3]>,
        pub(super) intensity: Option<f32>,
        pub(super) range: Option<f32>,
        pub(super) spot: Option<Spot>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Spot {
        pub(super) inner_cone_angle: Option<f32>,
        pub(super) outer_cone_angle: Option<f32>,
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
        pub(super) extensions: NodeExtensions,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Skin {
        pub(super) inverse_bind_matrices: Option<u32>,
        pub(super) joints: Vec<u32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Animation {
        pub(super) name: Option<String>,
        pub(super) channels: Vec<AnimationChannel>,
        pub(super) samplers: Vec<AnimationSampler>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct AnimationChannel {
        pub(super) sampler: u32,
        pub(super) target: AnimationTarget,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct AnimationTarget {
        pub(super) node: Option<u32>,
        pub(super) path: String,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct AnimationSampler {
        pub(super) input: u32,
        pub(super) interpolation: Option<String>,
        pub(super) output: u32,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub(super) struct NodeExtensions {
        #[serde(rename = "KHR_lights_punctual")]
        pub(super) lights_punctual: Option<NodeLight>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct NodeLight {
        pub(super) light: u32,
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
        pub(super) normal_texture: Option<NormalTextureInfo>,
        pub(super) occlusion_texture: Option<OcclusionTextureInfo>,
        pub(super) emissive_texture: Option<TextureInfo>,
        pub(super) emissive_factor: Option<[f32; 3]>,
        pub(super) alpha_mode: Option<String>,
        pub(super) alpha_cutoff: Option<f32>,
        pub(super) double_sided: bool,
        pub(super) extensions: MaterialExtensions,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub(super) struct MaterialExtensions {
        #[serde(rename = "KHR_materials_unlit")]
        pub(super) unlit: Option<IgnoredAny>,
        #[serde(rename = "KHR_materials_emissive_strength")]
        pub(super) emissive_strength: Option<EmissiveStrength>,
        #[serde(rename = "KHR_materials_pbrSpecularGlossiness")]
        pub(super) specular_glossiness: Option<SpecularGlossiness>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct SpecularGlossiness {
        pub(super) diffuse_factor: Option<[f32; 4]>,
        pub(super) diffuse_texture: Option<TextureInfo>,
        pub(super) specular_factor: Option<[f32; 3]>,
        pub(super) glossiness_factor: Option<f32>,
        pub(super) specular_glossiness_texture: Option<TextureInfo>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct EmissiveStrength {
        pub(super) emissive_strength: Option<f32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Pbr {
        pub(super) base_color_factor: Option<[f32; 4]>,
        pub(super) base_color_texture: Option<TextureInfo>,
        pub(super) metallic_factor: Option<f32>,
        pub(super) roughness_factor: Option<f32>,
        pub(super) metallic_roughness_texture: Option<TextureInfo>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct TextureInfo {
        pub(super) index: u32,
        pub(super) tex_coord: u32,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct NormalTextureInfo {
        #[serde(flatten)]
        pub(super) info: TextureInfo,
        pub(super) scale: Option<f32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct OcclusionTextureInfo {
        #[serde(flatten)]
        pub(super) info: TextureInfo,
        pub(super) strength: Option<f32>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct Texture {
        pub(super) sampler: Option<u32>,
        pub(super) source: Option<u32>,
        pub(super) extensions: TextureExtensions,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    pub(super) struct TextureExtensions {
        #[serde(rename = "MSFT_texture_dds")]
        pub(super) dds: Option<TextureDds>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    pub(super) struct TextureDds {
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
        let normals = bin.view(&floats(&[0., 0., 1., 0., 0., 1., 0., 0., 1.]), None);
        let gltf = json!({
            "asset": {"version": "2.0", "generator": "a test"},
            "extensionsUsed": [
                "KHR_materials_unlit",
                "KHR_texture_transform",
                "KHR_lights_punctual",
                "KHR_materials_emissive_strength",
            ],
            "extensionsRequired": ["KHR_materials_unlit"],
            "extensions": {"KHR_lights_punctual": {"lights": [
                {"type": "point", "color": [1, 0.5, 0.25], "intensity": 40, "range": 6},
            ]}},
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
                // The first mesh again, and a light.
                {
                    "translation": [7, 0, 0],
                    "mesh": 0,
                    "extensions": {"KHR_lights_punctual": {"light": 0}},
                },
            ],
            "meshes": [
                {"name": "pair", "primitives": [
                    {
                        "attributes": {"POSITION": 0, "TEXCOORD_0": 1, "NORMAL": 7},
                        "indices": 2,
                        "material": 0,
                    },
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
                {"bufferView": normals, "componentType": 5126, "count": 3, "type": "VEC3"},
            ],
            "materials": [
                {
                    "pbrMetallicRoughness": {
                        "baseColorFactor": [1, 0.5, 0.25, 1],
                        "baseColorTexture": {"index": 0},
                        "metallicFactor": 0.25,
                        "roughnessFactor": 0.5,
                    },
                    // Of the image that its base color samples as colors, so left out.
                    "occlusionTexture": {"index": 1, "strength": 0.5},
                    "emissiveFactor": [1, 0.5, 0],
                    "extensions": {
                        "KHR_materials_emissive_strength": {"emissiveStrength": 2},
                        "KHR_materials_unlit": {},
                    },
                    "alphaMode": "MASK",
                    "alphaCutoff": 0.25,
                },
                {
                    "pbrMetallicRoughness": {"baseColorTexture": {"index": 1, "texCoord": 0}},
                    "normalTexture": {"index": 2, "scale": 0.5},
                    // Of another set of texture coordinates, so left out.
                    "emissiveTexture": {"index": 1, "texCoord": 1},
                    "alphaMode": "BLEND",
                    "doubleSided": true,
                },
            ],
            // Two textures of one image, and one of a second.
            "textures": [{"source": 0, "sampler": 0}, {"source": 0}, {"source": 1}],
            "samplers": [{"magFilter": 9728, "minFilter": 9986, "wrapS": 33071, "wrapT": 33071}],
            // Said to be a JPEG, but a PNG, as its bytes say; and the same PNG again.
            "images": [{"bufferView": image, "mimeType": "image/jpeg"}, {"bufferView": image}],
            "buffers": [{"byteLength": bin.bytes.len()}],
            "bufferViews": bin.views,
            "animations": [{"channels": [], "samplers": []}],
            "cameras": [{"type": "perspective"}, {"type": "orthographic"}],
        });
        glb(&gltf.to_string(), &bin.bytes)
    }

    /// A model reads into its parts, each node's light among them; its primitives, with their
    /// normals, or made flat without them, and their tangents, made by MikkTSpace for a normal
    /// map's, and their materials, with each texture and factor and the extensions the engine
    /// reads; and its images; leaving out what it says it leaves out.
    #[test]
    fn a_model_reads_into_parts_primitives_and_images() {
        let imported = read_glb(&model()).expect("a model");
        let part = |transform, parent, primitive| Part {
            transform,
            parent,
            primitive,
            light: None,
            skin: None,
        };
        let lit = |transform, parent, primitive| Part {
            light: Some(Light {
                kind: LightKind::Point,
                color: Vec3::new(1.0, 0.5, 0.25),
                intensity: 40.0,
                range: Some(6.0),
                ..Light::default()
            }),
            ..part(transform, parent, primitive)
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
                lit(moved(7.0, 0.0, 0.0), Some(0), None),
                part(identity, Some(4), Some(0)),
                part(identity, Some(4), Some(1)),
            ]
        );
        let triangle = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let white = vec![[1.0; 4]; 3];
        let half = 32768.0 / 65535.0;
        let (front, back) = (vec![[0.0, 0.0, 1.0]; 3], vec![[0.0, 0.0, -1.0]; 3]);
        let unused = vec![[1.0, 0.0, 0.0, 1.0]; 3];
        let sampled = |image, nearest, wrap| {
            Some(Sampling {
                image,
                nearest,
                wrap,
            })
        };
        let mut primitives = imported.primitives.clone();
        // MikkTSpace's, along +X, the bitangent cross(normal, tangent) * W along +Y, as V runs up
        // the triangle facing -Z: compared within rounding, then as exactly that.
        let made = &mut primitives[1].tangents;
        for tangent in made.iter() {
            let near = Vec4::from(*tangent).abs_diff_eq(Vec4::new(1.0, 0.0, 0.0, -1.0), 1e-6);
            assert!(near, "{tangent:?}");
        }
        *made = vec![[1.0, 0.0, 0.0, -1.0]; 3];
        assert_eq!(
            primitives,
            [
                Primitive {
                    name: "pair/0".to_owned(),
                    positions: triangle.clone(),
                    uvs: uvs.clone(),
                    colors: white,
                    normals: front.clone(),
                    tangents: unused.clone(),
                    indices: vec![0, 1, 2],
                    mesh: None,
                    material: Material {
                        color: [1.0, 0.5, 0.25, 1.0],
                        workflow: Workflow::MetallicRoughness {
                            metallic: 0.25,
                            roughness: 0.5,
                        },
                        occlusion_strength: 0.5,
                        emissive: [2.0, 1.0, 0.0],
                        textures: [sampled(0, true, Wrap::Clamp), None, None, None, None],
                        alpha: AlphaMode::Mask { cutoff: 0.25 },
                        unlit: true,
                        ..Material::default()
                    },
                    joints: Vec::new(),
                    weights: Vec::new(),
                    reach: None,
                },
                // Without normals, so flat: each triangle's corners its own, in its order.
                Primitive {
                    name: "pair/1".to_owned(),
                    positions: triangle.into_iter().rev().collect(),
                    uvs: uvs.into_iter().rev().collect(),
                    colors: vec![
                        [0.0, 0.0, 1.0, 1.0],
                        [0.0, 1.0, 0.0, 1.0],
                        [1.0, 0.0, 0.0, 1.0],
                    ],
                    normals: back,
                    tangents: vec![[1.0, 0.0, 0.0, -1.0]; 3],
                    indices: vec![0, 1, 2],
                    mesh: None,
                    material: Material {
                        normal_scale: 0.5,
                        textures: [
                            sampled(0, false, Wrap::Repeat),
                            None,
                            sampled(1, false, Wrap::Repeat),
                            None,
                            None,
                        ],
                        alpha: AlphaMode::Blend,
                        double_sided: true,
                        ..Material::default()
                    },
                    joints: Vec::new(),
                    weights: Vec::new(),
                    reach: None,
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
                    normals: front,
                    tangents: unused,
                    indices: vec![0, 1, 2],
                    mesh: None,
                    material: Material::default(),
                    joints: Vec::new(),
                    weights: Vec::new(),
                    reach: None,
                },
            ]
        );
        // Read as far as their headers, the PNG they are, which decodes as the loader decodes it.
        let [colors, normals] = &imported.images[..] else {
            panic!("two images: {:?}", imported.images);
        };
        for read in [colors, normals] {
            assert_eq!((read.size, read.format), ([2, 1], image::Format::Png));
        }
        let image = Image {
            size: [2, 1],
            pixels: PIXELS.to_vec(),
            format: image::Format::Rgba8,
        };
        let mut decoded = imported.clone();
        decoded
            .decode_images("m", Path::new("m.glb"))
            .expect("its images decode");
        assert_eq!(decoded.images, [image.clone(), image]);
        // Its animation of no channels, a clip that moves nothing, named by its index.
        let clips: Vec<(&str, f32, usize)> = (imported.clips.iter())
            .map(|clip| (clip.name.as_str(), clip.duration, clip.channels.len()))
            .collect();
        assert_eq!(clips, [("0", 0.0, 0)]);
        assert_eq!(
            imported.ignored,
            [
                "2 cameras",
                "extension KHR_texture_transform",
                "material 0's occlusionTexture, which samples image 0 as values where another \
                 texture samples it as colors",
                "material 1's emissiveTexture, which reads TEXCOORD_1 where its others read \
                 TEXCOORD_0",
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
        let mut imported =
            read_files("m.gltf", gltf.to_string().as_bytes(), &files).expect("a model");
        let decoded = imported.decode_images("m", Path::new("m.gltf"));
        decoded.expect("its image decodes");
        let part = Part {
            transform: Transform::IDENTITY,
            parent: None,
            primitive: Some(0),
            light: None,
            skin: None,
        };
        assert_eq!(imported.parts, [part]);
        // Without normals, so flat: each corner its own, in the triangle's order.
        let primitive = Primitive {
            name: "triangle/0".to_owned(),
            positions: vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
            uvs: vec![[0.0, 0.0], [0.0, 1.0], [1.0, 0.0]],
            colors: vec![[1.0; 4]; 3],
            normals: vec![[0.0, 0.0, -1.0]; 3],
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
            indices: vec![0, 1, 2],
            mesh: None,
            material: Material {
                textures: [
                    Some(Sampling {
                        image: 0,
                        nearest: false,
                        wrap: Wrap::Repeat,
                    }),
                    None,
                    None,
                    None,
                    None,
                ],
                ..Material::default()
            },
            joints: Vec::new(),
            weights: Vec::new(),
            reach: None,
        };
        assert_eq!(imported.primitives, [primitive]);
        assert_eq!(imported.images[0].pixels, PIXELS);

        // From the disk, listing the files read, its own first, as hot reload watches them.
        let scratch = crate::dirs::tests::Scratch::new("gltf-files");
        let folder = scratch.0.join("models");
        std::fs::create_dir_all(folder.join("textures")).expect("a directory");
        let model = folder.join("m.gltf");
        std::fs::write(&model, gltf.to_string()).expect("the model");
        for (name, bytes) in files {
            std::fs::write(folder.join(name), bytes).expect("a file it names");
        }
        let (mut read, listed) = read_listing("m", &model, &Files::default()).expect("a model");
        let decoded = read.decode_images("m", &model);
        decoded.expect("its image decodes");
        assert_eq!(read, imported);
        let mut named: Vec<PathBuf> = listed[1..].to_vec();
        named.sort();
        let beside = ["my data.bin", "textures/a b.png"].map(|name| folder.join(name));
        assert_eq!((&listed[0], named.as_slice()), (&model, beside.as_slice()));
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

    /// A model of a triangle without normals, so drawn flat, that a skin of two joints moves, a
    /// root's and its child's, a unit along X: its first corner the root's alone, its second half
    /// each, its third the child's; and an animation, "Turn", that turns the child a quarter about
    /// Z over a second, along a line, lifts the root two units at once at half a second, by steps,
    /// and moves node 3, outside the scene.
    fn skinned() -> (Value, Vec<u8>) {
        let mut bin = Bin::default();
        let positions = bin.view(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]), None);
        let joints = bin.view(&[0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0], None);
        let weights = floats(&[1., 0., 0., 0., 0.5, 0.5, 0., 0., 1., 0., 0., 0.]);
        let weights = bin.view(&weights, None);
        let mut binds = Mat4::IDENTITY.to_cols_array().to_vec();
        binds.extend(Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0)).to_cols_array());
        let binds = bin.view(&floats(&binds), None);
        let times = bin.view(&floats(&[0.0, 1.0]), None);
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let turns = bin.view(&floats(&[0., 0., 0., 1., 0., 0., half, half]), None);
        let steps = bin.view(&floats(&[0.0, 0.5]), None);
        let lifts = bin.view(&floats(&[0., 0., 0., 0., 2., 0.]), None);
        let gltf = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0, 2]}],
            "nodes": [
                {"children": [1]},
                {"translation": [1.0, 0.0, 0.0]},
                {"mesh": 0, "skin": 0},
                {},
            ],
            "meshes": [{"primitives": [{
                "attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2},
            }]}],
            "skins": [{"joints": [0, 1], "inverseBindMatrices": 3}],
            "animations": [{
                "name": "Turn",
                "channels": [
                    {"sampler": 0, "target": {"node": 1, "path": "rotation"}},
                    {"sampler": 1, "target": {"node": 0, "path": "translation"}},
                    {"sampler": 1, "target": {"node": 3, "path": "translation"}},
                ],
                "samplers": [
                    {"input": 4, "output": 5},
                    {"input": 6, "output": 7, "interpolation": "STEP"},
                ],
            }],
            "accessors": [
                {"bufferView": positions, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": joints, "componentType": 5121, "count": 3, "type": "VEC4"},
                {"bufferView": weights, "componentType": 5126, "count": 3, "type": "VEC4"},
                {"bufferView": binds, "componentType": 5126, "count": 2, "type": "MAT4"},
                {"bufferView": times, "componentType": 5126, "count": 2, "type": "SCALAR"},
                {"bufferView": turns, "componentType": 5126, "count": 2, "type": "VEC4"},
                {"bufferView": steps, "componentType": 5126, "count": 2, "type": "SCALAR"},
                {"bufferView": lifts, "componentType": 5126, "count": 2, "type": "VEC3"},
            ],
            "buffers": [{"byteLength": bin.bytes.len()}],
            "bufferViews": bin.views,
        });
        (gltf, bin.bytes)
    }

    /// A skinned model reads its skin, its joints' parts and their inverse bind matrices; its
    /// primitive's joints and weights, which follow its corners as it's made flat; its skinned
    /// part's skin; and its animation's channels of nodes in its scene, as clips, sampled along
    /// their keys, leaving out the one of a node outside it. Its primitive reaches as far as its
    /// joints carry it in every pose, with room to spare.
    #[test]
    fn a_skinned_model_reads_its_skin_its_weights_and_its_clips() {
        let (gltf, bin) = skinned();
        let imported = read_glb(&glb(&gltf.to_string(), &bin)).expect("a skinned model");
        let parts: Vec<(Option<usize>, Option<usize>, Option<usize>)> = (imported.parts.iter())
            .map(|part| (part.parent, part.primitive, part.skin))
            .collect();
        assert_eq!(
            parts,
            [
                (None, None, None),
                (Some(0), None, None),
                (None, Some(0), Some(0))
            ]
        );
        let unbound = Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0));
        let skin = Skin {
            joints: vec![0, 1],
            inverse_binds: vec![Mat4::IDENTITY, unbound],
        };
        assert_eq!(imported.skins, [skin]);
        let primitive = &imported.primitives[0];
        assert_eq!(primitive.joints, [[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]]);
        let weights = [
            [1.0, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
        ];
        assert_eq!(primitive.weights, weights);
        assert_eq!(primitive.normals, [[0.0, 0.0, 1.0]; 3], "flat");
        let [clip] = &imported.clips[..] else {
            panic!("a clip: {:?}", imported.clips);
        };
        assert_eq!((clip.name.as_str(), clip.duration), ("Turn", 1.0));
        let moved: Vec<(u32, Moved, Interpolation)> = (clip.channels.iter())
            .map(|channel| (channel.part, channel.path, channel.interpolation))
            .collect();
        let (rotation, translation) = (Moved::Rotation, Moved::Translation);
        let (linear, step) = (Interpolation::Linear, Interpolation::Step);
        assert_eq!(moved, [(1, rotation, linear), (0, translation, step)]);
        let eighth = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
        let turned = Quat::from_vec4(clip.channels[0].sample(0.5));
        assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
        let lift = |time| clip.channels[1].sample(time).truncate();
        assert_eq!(
            [lift(0.49), lift(0.5), lift(9.0)],
            [Vec3::ZERO, Vec3::Y * 2.0, Vec3::Y * 2.0]
        );
        let ignored = "channel 2 of animation 0, of node 3 outside the scene";
        assert_eq!(imported.ignored, [ignored]);
        // Lifted two at half a second, its first corner at (0, 2) and its second at (1, 2); and
        // its third, the child turned an eighth, at (1 - √2, 2): within its reach, which spares a
        // tenth of its longest side, 2, each way.
        let [low, high] = primitive.reach.expect("a skinned primitive reaches");
        let turned = Vec3::new(1.0 - std::f32::consts::SQRT_2, 2.0, 0.0);
        let held = [Vec3::ZERO, Vec3::Y * 2.0, Vec3::new(1.0, 2.0, 0.0), turned];
        for point in held {
            assert!(
                point.cmpge(low).all() && point.cmple(high).all(),
                "{point} in {low}..{high}"
            );
        }
        assert!(
            (low - Vec3::new(turned.x - 0.2, -0.2, -0.2))
                .abs()
                .max_element()
                < 1e-5,
            "{low}"
        );
        assert!(
            (high - Vec3::new(1.2, 2.2, 0.2)).abs().max_element() < 1e-5,
            "{high}"
        );
    }

    /// A skin, a skinned primitive or a clip that doesn't hold together refuses the model, saying
    /// how.
    #[test]
    fn a_skin_or_clip_that_doesnt_hold_together_is_refused() {
        let (gltf, bin) = skinned();
        type Change = fn(&mut Value);
        let cases: [(&str, Change); 7] = [
            ("one of JOINTS_0 and WEIGHTS_0 without the other", |g| {
                remove(
                    &mut g["meshes"][0]["primitives"][0]["attributes"],
                    "WEIGHTS_0",
                );
            }),
            ("without JOINTS_0 and WEIGHTS_0", |g| {
                let attributes = &mut g["meshes"][0]["primitives"][0]["attributes"];
                remove(attributes, "WEIGHTS_0");
                remove(attributes, "JOINTS_0");
            }),
            ("moved by joint 1, past the 1 of skin 0", |g| {
                g["skins"][0]["joints"] = json!([0])
            }),
            ("of joint node 3, which isn't in its scene", |g| {
                g["skins"][0]["joints"] = json!([0, 3]);
            }),
            ("of 2 joints and 1 inverse bind matrices", |g| {
                g["accessors"][3]["count"] = json!(1)
            }),
            ("of times that don't go up from 0", |g| {
                g["accessors"][4]["bufferView"] = json!(5)
            }),
            ("of 2 times and 1 values", |g| {
                g["accessors"][7]["count"] = json!(1)
            }),
        ];
        for (problem, change) in cases {
            let mut gltf = gltf.clone();
            change(&mut gltf);
            let error = read_glb(&glb(&gltf.to_string(), &bin)).expect_err(problem);
            assert_eq!(error.code(), "engine.bad_model", "{problem}: {error}");
            assert!(error.to_string().contains(problem), "{problem}: {error}");
        }
    }

    /// A `.gltf` of [`triangle`]'s model, its buffer in a data URI, with `edit` made to its JSON:
    /// for tests that edit a model's file as people do.
    #[cfg(feature = "dev")]
    pub(crate) fn triangle_gltf(edit: impl FnOnce(&mut Value)) -> String {
        let (mut gltf, bin) = triangle();
        let uri = format!("data:application/octet-stream;base64,{}", encode(&bin));
        gltf["buffers"][0]["uri"] = json!(uri);
        edit(&mut gltf);
        gltf.to_string()
    }

    /// Removes `key` from the object `value`.
    fn remove(value: &mut Value, key: &str) {
        value.as_object_mut().expect("an object").remove(key);
    }

    /// A material of KHR_materials_pbrSpecularGlossiness's is that, its diffuse color and texture
    /// in place of a base color's and its specular-glossiness map, which holds colors, in place of
    /// its metallic-roughness, the extension's fallback, which is ignored. A texture whose
    /// MSFT_texture_dds names a DDS image samples that, not its source, which isn't there: a DDS of
    /// BC7, read as far as its headers. Neither extension is ignored; a DDS that isn't of BC7 is
    /// refused, naming its image.
    #[test]
    fn specular_glossiness_and_dds_images_read_as_their_extensions_say() {
        let (mut gltf, bin) = triangle();
        gltf["extensionsUsed"] = json!(["KHR_materials_pbrSpecularGlossiness", "MSFT_texture_dds"]);
        gltf["materials"] = json!([{
            "pbrMetallicRoughness": {"baseColorFactor": [0.0, 0.0, 0.0, 1.0], "metallicFactor": 0.5},
            "extensions": {"KHR_materials_pbrSpecularGlossiness": {
                "diffuseFactor": [1.0, 0.5, 0.25, 1.0],
                "diffuseTexture": {"index": 0},
                "specularFactor": [0.5, 0.25, 0.125],
                "glossinessFactor": 0.75,
                "specularGlossinessTexture": {"index": 1},
            }},
        }]);
        let dds_of = |source: u32| json!({"source": 1, "extensions": {"MSFT_texture_dds": {"source": source}}});
        gltf["textures"] = json!([dds_of(2), dds_of(3)]);
        let png = gltf["images"][0].clone();
        gltf["images"] = json!([png, {"uri": "missing.png"}, {"uri": "a.dds"}, {"uri": "b.dds"}]);
        gltf["buffers"][0]["uri"] = json!("m.bin");
        let read = |gltf: &Value, b: &[u8]| {
            let a = crate::image::tests::dds(99, [4, 4], &[vec![1; 16]]);
            let files = [("m.bin", &bin[..]), ("a.dds", &a[..]), ("b.dds", b)];
            read_files("m.gltf", gltf.to_string().as_bytes(), &files)
        };
        let b = crate::image::tests::dds(98, [4, 4], &[vec![2; 16]]);
        let imported = read(&gltf, &b).expect("a model");
        let material = imported.primitives[0].material;
        assert_eq!(material.color, [1.0, 0.5, 0.25, 1.0]);
        let workflow = Workflow::SpecularGlossiness {
            specular: Vec3::new(0.5, 0.25, 0.125),
            glossiness: 0.75,
        };
        assert_eq!(material.workflow, workflow);
        let sampled = material
            .textures
            .map(|texture| texture.map(|texture| texture.image));
        assert_eq!(sampled, [Some(0), Some(1), None, None, None]);
        assert_eq!(material.map(Slot::Surface), None, "colors");
        let dds = |compressed| crate::image::Format::Dds {
            compressed,
            mipmapped: false,
        };
        let formats: Vec<_> = (imported.images.iter())
            .map(|image| (image.size, image.format))
            .collect();
        use crate::image::Compressed::{Bc7Srgb, Bc7Unorm};
        assert_eq!(formats, [([4, 4], dds(Bc7Srgb)), ([4, 4], dds(Bc7Unorm))]);
        assert!(imported.ignored.is_empty(), "{:?}", imported.ignored);
        let bc1 = crate::image::tests::dds(71, [4, 4], &[vec![2; 8]]);
        let error = read(&gltf, &bc1).expect_err("BC1");
        assert_eq!(error.code(), "engine.bad_model");
        let message = error.to_string();
        assert!(
            message.contains("image 3") && message.contains("DXGI format 71"),
            "{message}"
        );
    }

    /// A primitive whose TANGENT holds numbers that aren't finite, as a tool can leave them where a
    /// triangle's texture coordinates have no area, is drawn as one without tangents, its normal
    /// map's from MikkTSpace, which `ignored` says; its other attributes still may hold none.
    #[test]
    fn tangents_that_arent_finite_are_ignored() {
        let (mut gltf, mut bin) = triangle();
        let mut view = |bytes: &[u8]| {
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            let at = bin.len();
            bin.extend_from_slice(bytes);
            let views = gltf["bufferViews"].as_array_mut().expect("views");
            views.push(json!({"buffer": 0, "byteOffset": at, "byteLength": bytes.len()}));
            views.len() - 1
        };
        let nan = f32::NAN;
        let normals = view(&floats(&[0., 0., 1., 0., 0., 1., 0., 0., 1.]));
        let tangents = view(&floats(&[
            1., 0., 0., 1., nan, nan, nan, 0., 1., 0., 0., 1.,
        ]));
        let nans = view(&floats(&[nan; 9]));
        gltf["buffers"][0]["byteLength"] = json!(bin.len());
        let accessors = gltf["accessors"].as_array_mut().expect("accessors");
        for (view, kind) in [(normals, "VEC3"), (tangents, "VEC4"), (nans, "VEC3")] {
            accessors
                .push(json!({"bufferView": view, "componentType": 5126, "count": 3, "type": kind}));
        }
        let attributes = &mut gltf["meshes"][0]["primitives"][0]["attributes"];
        attributes["NORMAL"] = json!(3);
        attributes["TANGENT"] = json!(4);
        // Its own image, of the same PNG, which the base color samples as colors.
        let png = gltf["images"][0].clone();
        gltf["images"].as_array_mut().expect("images").push(png);
        let texture = json!({"source": 1});
        gltf["textures"]
            .as_array_mut()
            .expect("textures")
            .push(texture);
        gltf["materials"][0]["normalTexture"] = json!({"index": 1});
        let read = |gltf: &Value| read_glb(&glb(&gltf.to_string(), &bin));
        let imported = read(&gltf).expect("a model");
        let ignored = "primitive 0 of mesh 0's TANGENT, which holds numbers that aren't finite: \
                       it's drawn as without one";
        assert_eq!(imported.ignored, [ignored]);
        // MikkTSpace's, along U, which runs along X.
        let made = &imported.primitives[0].tangents;
        assert!(
            made.iter().all(|tangent| tangent == &[1.0, 0.0, 0.0, 1.0]),
            "{made:?}"
        );
        gltf["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(5);
        let error = read(&gltf).expect_err("normals that aren't finite");
        assert!(error.to_string().contains("isn't finite"), "{error}");
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
            // A rotation so near 0 that its length, squared, loses its precision, which then
            // isn't a unit as the world takes one.
            (bad, "rotation is 0", |g| {
                g["nodes"][0]["rotation"] = json!([2e-22, 0, 0, 0])
            }),
            // A number past an f32's range, which `float_roundtrip` refuses, where it would read
            // as infinite; and an emissive factor within it, times a strength, past it.
            (bad, "isn't glTF's JSON", |g| {
                g["nodes"][0]["translation"] = json!([1e39, 0, 0]);
            }),
            (bad, "material 0 has a number that isn't finite", |g| {
                g["materials"][0]["emissiveFactor"] = json!([3e38, 0, 0]);
                g["materials"][0]["extensions"] = json!({
                    "KHR_materials_emissive_strength": {"emissiveStrength": 2},
                });
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

    /// What reading holds on the way counts against the memory it may take, as it's about to be
    /// made, as well as what it holds once read: the indices as they're read, beside the
    /// primitive's copy, and the vertices of a primitive drawn flat, or parted for MikkTSpace's
    /// tangents, beside those they're made from, with MikkTSpace's tangent for each corner. A
    /// primitive's file holds a byte for each of its corners, which are made each a vertex of 64
    /// bytes, so that a gigabyte of file would otherwise take some 70 before it was counted.
    #[test]
    fn what_reading_holds_on_the_way_counts_against_the_memory_it_may_take() {
        // A triangle drawn 1,000 times, by indices of a byte each: flat, for want of normals; or
        // with normals and a normal map, without tangents.
        let model = |normal_map: bool| {
            let mut bin = Bin::default();
            let positions = bin.view(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]), None);
            let normals = bin.view(&floats(&[0., 0., 1., 0., 0., 1., 0., 0., 1.]), None);
            let uvs = bin.view(&floats(&[0., 0., 1., 0., 0., 1.]), None);
            let indices = bin.view(&[0, 1, 2].repeat(1_000), None);
            let image = bin.view(&png(), None);
            let accessor = |view, kind, count, component| json!({"bufferView": view, "type": kind, "count": count, "componentType": component});
            let mut gltf = json!({
                "asset": {"version": "2.0"},
                "scenes": [{"nodes": [0]}],
                "nodes": [{"mesh": 0}],
                "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 3}]}],
                "accessors": [
                    accessor(positions, "VEC3", 3, FLOAT),
                    accessor(normals, "VEC3", 3, FLOAT),
                    accessor(uvs, "VEC2", 3, FLOAT),
                    accessor(indices, "SCALAR", 3_000, UNSIGNED_BYTE),
                ],
                "buffers": [{"byteLength": bin.bytes.len()}],
                "bufferViews": bin.views,
            });
            if normal_map {
                let primitive = &mut gltf["meshes"][0]["primitives"][0];
                primitive["attributes"] = json!({"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2});
                primitive["material"] = json!(0);
                gltf["materials"] = json!([{"normalTexture": {"index": 0}}]);
                gltf["textures"] = json!([{"source": 0}]);
                gltf["images"] = json!([{"bufferView": image}]);
            }
            glb(&gltf.to_string(), &bin.bytes)
        };
        // What a model holds once read: its parts, its primitives' vertices, indices and names,
        // and its images' files and what they decode to.
        let held = |imported: &Imported| {
            let primitives = (imported.primitives.iter())
                .map(|primitive| primitive.held() + primitive.name.len());
            let images =
                (imported.images.iter()).map(|image| image.pixels.len() + image.decoded_len());
            (size_of_val(imported.parts.as_slice()) + primitives.chain(images).sum::<usize>())
                as u64
        };
        for normal_map in [false, true] {
            let model = model(normal_map);
            let read_with =
                |memory| read("m", Path::new("m.glb"), |_, _| Ok(model.clone()), memory);
            let imported = read_with(MAX_MEMORY).expect("a model");
            assert_eq!(imported.primitives[0].indices.len(), 3_000);
            let error = read_with(held(&imported)).expect_err("past what it holds once read");
            assert_eq!(error.code(), "engine.unsupported_model", "{error}");
        }
    }
}
