//! A model's meshes: their primitives' vertices and indices, drawn flat or given tangents.

use std::collections::BTreeMap;
use std::io;
use std::ops::Range;
use std::path::Path;

use bevy_mikktspace::{Geometry, Ops, TangentSpace};
use gg_core::Result;
use gg_core::math::{Vec3, libm};
use gg_renderer::{AlphaMode, ModelMesh};

use super::material::{Material, Slot};
use super::{Memory, Reader, json};

/// Triangles, with what their vertices and their material give them.
#[derive(Clone, Debug, PartialEq)]
pub struct Primitive {
    /// The mesh's name or index, then the primitive's index: "Cesium_Milk_Truck/2".
    pub(crate) name: String,
    pub positions: Vec<[f32; 3]>,
    /// As many as the positions: (0, 0) where the material samples no texture.
    pub uvs: Vec<[f32; 2]>,
    /// As many as the positions, linear RGBA: white without COLOR_0, alpha 1 for a VEC3.
    pub colors: Vec<[f32; 4]>,
    /// As many as the positions, unit vectors: its NORMALs. Without them, each triangle's own
    /// with its corners apart from its neighbors' (glTF's flat drawing); +Z (unread) for an unlit
    /// material.
    pub normals: Vec<[f32; 3]>,
    /// As many as the positions: its TANGENTs, W the bitangent's sign. Without them, for a lit
    /// material with a normal map, MikkTSpace's (as glTF says), vertices apart where their
    /// corners' tangents differ; otherwise (1, 0, 0, 1), unread.
    pub tangents: Vec<[f32; 4]>,
    /// With JOINTS_0 and WEIGHTS_0, as many as the positions: each vertex's four joints, of the
    /// skin that moves it. Empty otherwise.
    pub joints: Vec<[u16; 4]>,
    /// As many as `joints`: how much each moves its vertex.
    pub weights: Vec<[f32; 4]>,
    /// For a skinned node's primitive, the box (least, most corner) its vertices reach as the
    /// skins' joints move them through the model's clips, with room to spare. Its mesh packs
    /// within it and views cull by it.
    pub reach: Option<[Vec3; 2]>,
    /// Three for each triangle; 0..count where the primitive has none.
    pub indices: Vec<u32>,
    /// Its mesh as the cooker made it: triangles optimized, with levels of detail and meshlets,
    /// vertices packed. A run from a package reads it instead of the then-empty vertices and
    /// indices above. None as read from glTF; a run packs it when it adds the primitive.
    pub mesh: Option<ModelMesh>,
    pub(crate) material: Material,
}

impl Primitive {
    /// Whether its material blends: its triangles cover each other in draw order, which the cooker
    /// keeps.
    pub fn blends(&self) -> bool {
        matches!(self.material.alpha, AlphaMode::Blend)
    }

    /// The bytes its vertices and indices take, as [`Memory`] counts them.
    pub(super) fn held(&self) -> usize {
        size_of_val(self.positions.as_slice())
            + size_of_val(self.uvs.as_slice())
            + size_of_val(self.colors.as_slice())
            + size_of_val(self.normals.as_slice())
            + size_of_val(self.tangents.as_slice())
            + size_of_val(self.joints.as_slice())
            + size_of_val(self.weights.as_slice())
            + size_of_val(self.indices.as_slice())
    }

    /// The bytes a vertex's joints and weights take: none without them.
    fn skin_bytes(&self) -> usize {
        if self.joints.is_empty() {
            0
        } else {
            size_of::<[u16; 4]>() + size_of::<[f32; 4]>()
        }
    }

    /// Gives each triangle corners of its own, facing as it does (front counterclockwise): glTF's
    /// flat drawing without normals. A triangle of no area faces +Z, covering nothing that shows.
    /// Its tangents go; it has none before it has normals.
    ///
    /// Fails, changing nothing, if `memory` lacks room for the new vertices beside the old.
    fn make_flat(&mut self, memory: &mut Memory) -> Result<()> {
        fn corners<T: Copy>(indices: &[u32], values: &[T]) -> Vec<T> {
            indices.iter().map(|&i| values[i as usize]).collect()
        }
        // A vertex a corner, less its tangent, plus its index.
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

    /// Gives it MikkTSpace's tangents from its positions, normals and texture coordinates, as
    /// glTF says for a normal-mapped primitive without them: a tangent per triangle corner, each
    /// vertex split per distinct tangent of its corners, in the order the corners first reach
    /// them. A corner MikkTSpace gives none (only in a triangle of no area) takes +X.
    ///
    /// Fails, changing nothing, if `memory` lacks room for the tangents and split vertices beside
    /// the old.
    fn make_tangents(&mut self, memory: &mut Memory) -> Result<()> {
        let count = self.indices.len();
        // Each corner's tangent from MikkTSpace and index, and its vertex and tangent if split
        // (at most one a corner).
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
        // Frees the old vertices and indices, MikkTSpace's tangents and the placed vertices; the
        // new indices stay.
        memory.release(before + made - count * size_of::<u32>());
        Ok(())
    }
}

/// The bytes a [`Primitive`] vertex takes: position, texture coordinates, color, normal, tangent.
const VERTEX: usize = size_of::<[f32; 3]>() * 2 + size_of::<[f32; 2]>() + size_of::<[f32; 4]>() * 2;

/// The most bytes an entry of the `BTreeMap` placing [`Primitive::make_tangents`]' vertices takes,
/// with its share of the nodes. std's nodes hold 5 to 11 entries (the root fewer) beside a header
/// and an internal node's edges: at most about 2.5 times the entry, once a node's worth is held.
const PLACED: usize = 3 * size_of::<((u32, [u32; 4]), u32)>();

/// A primitive's triangles as MikkTSpace reads them, and the tangent it gives each corner.
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
            // MikkTSpace's bitangent runs with V, down glTF's images, but normal maps' green runs
            // up them: W flips, as three.js' `negateSign` and Blender's flipped V do.
            let [x, y, z, w] = tangent.tangent_encoded();
            self.tangents[face * 3 + corner] = [x, y, z, -w];
        }
    }
}

/// MikkTSpace's square roots and arccosines, from libm like the engine's, so a model's tangents
/// are the same on every platform, from its files or cooked.
struct Libm;

impl Ops for Libm {
    fn sqrt(x: f32) -> f32 {
        libm::sqrtf(x)
    }

    fn acos(x: f32) -> f32 {
        libm::acosf(x)
    }
}

const TRIANGLES: u32 = 4;

impl<'a, F: Fn(&Path, u64) -> io::Result<Vec<u8>>> Reader<'a, F> {
    /// Reads mesh `index`, once. Gives where its primitives are in `imported.primitives`.
    pub(super) fn mesh(&mut self, index: u32) -> Result<Range<usize>> {
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

    /// Reads primitive `index` of `mesh` (`mesh_index`): `None` without positions, which glTF
    /// says not to draw.
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
        // Own copies, counted before they're made: primitives sharing accessors or a mesh's name
        // each have their own.
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
        // Tangents not all finite (a tool can leave them where a triangle's texture coordinates
        // have no area) are ignored: the primitive is drawn as without them.
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
        // Tangents without normals are ignored (glTF): the primitive is drawn flat. An unlit
        // material reads neither, so its vertices stay welded.
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
}
