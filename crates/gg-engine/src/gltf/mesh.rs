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

const TRIANGLES: u32 = 4;

impl<'a, F: Fn(&Path, u64) -> io::Result<Vec<u8>>> Reader<'a, F> {
    /// Reads mesh `index`, once: where the primitives it draws are in `imported.primitives`.
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
}
