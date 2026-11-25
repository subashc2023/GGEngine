//! The meshes a game adds, the handles it draws them by, and the changes a renderer takes.

use std::collections::HashSet;
use std::fmt;
use std::marker::PhantomData;

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3};
use gg_core::{Error, Result};

use crate::packed::{MeshletBounds, ModelMesh, PackedVertex};
use crate::shader::Shader;
#[cfg(doc)]
use crate::{Draws, Uploads};
use crate::{grid, sprite};

/// A handle [`Meshes::add`] handed out, of vertex type `V`.
pub struct Mesh<V> {
    pub(crate) index: u32,
    vertex: PhantomData<fn() -> V>,
}

// By hand, so `V` needn't be `Clone` or `Debug`.
impl<V> Clone for Mesh<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for Mesh<V> {}

impl<V> Mesh<V> {
    pub(crate) const fn new(index: u32) -> Self {
        Self {
            index,
            vertex: PhantomData,
        }
    }

    /// The order [`Meshes::add`] handed it out in, from 0 (the engine's quad's). [`MeshDraw`]s hold
    /// it.
    pub fn index(self) -> u32 {
        self.index
    }
}

impl<V> fmt::Debug for Mesh<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Mesh").field(&self.index).finish()
    }
}

/// A mesh, a shader and its parameters, as [`Draws::push`] draws them, without their types.
///
/// They are checked against each other at creation, then erased, so one type holds any draw, as in
/// a component. The parameters are copied in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshDraw {
    pub(crate) mesh: u32,
    pub(crate) shader: u32,
    /// Whether its shader writes depth.
    pub(super) opaque: bool,
    /// Whether it draws only its mesh's fronts, as a material that isn't double-sided does.
    pub(crate) culls: bool,
    pub(crate) params: [u8; MeshDraw::MAX_PARAMS],
    /// How many bytes of `params` are used.
    pub(crate) len: u8,
}

impl MeshDraw {
    /// The most parameter bytes it holds, until materials keep parameters on the GPU. A shader
    /// taking more draws through [`Draws::push`].
    pub const MAX_PARAMS: usize = 64;

    /// A draw of `mesh` with `shader`, which reads its vertex type, given `params`. Parameters of
    /// more than [`MAX_PARAMS`](Self::MAX_PARAMS) bytes don't compile.
    pub fn new<V, P: NoUninit>(mesh: Mesh<V>, shader: Shader<V, P>, params: P) -> Self {
        const {
            assert!(
                size_of::<P>() <= MeshDraw::MAX_PARAMS,
                "a MeshDraw holds parameters of up to 64 bytes"
            )
        };
        let bytes = bytemuck::bytes_of(&params);
        let mut copy = [0; Self::MAX_PARAMS];
        copy[..bytes.len()].copy_from_slice(bytes);
        Self {
            mesh: mesh.index,
            shader: shader.index,
            opaque: shader.opaque,
            culls: false,
            params: copy,
            len: bytes.len() as u8,
        }
    }

    /// The same draw of `mesh` instead, as a skinned instance's copy of its model's mesh. It must
    /// have the same vertex type, or its draws panic.
    pub fn with_mesh<V>(self, mesh: Mesh<V>) -> Self {
        Self {
            mesh: mesh.index,
            ..self
        }
    }

    /// Its batch key, if its shader is opaque: equal for draws of one mesh and shader, ordered by
    /// shader then mesh. Opaque draws hide by depth in any order, so sorting by it makes each run
    /// one instanced draw.
    pub fn batch(&self) -> Option<u64> {
        self.opaque
            .then(|| (u64::from(self.shader) << 32) | u64::from(self.mesh))
    }

    pub(crate) fn params(&self) -> &[u8] {
        &self.params[..usize::from(self.len)]
    }

    /// Its parts as a snapshot keeps them, for [`from_parts`](Self::from_parts): its mesh's and
    /// shader's indices, whether its shader is opaque, whether it culls, and its parameters' bytes.
    #[doc(hidden)]
    pub fn parts(&self) -> (u32, u32, bool, bool, &[u8]) {
        let parts = (self.mesh, self.shader, self.opaque, self.culls);
        (parts.0, parts.1, parts.2, parts.3, self.params())
    }

    /// A draw from [`parts`](Self::parts), for a renderer given the same meshes and shaders in the
    /// same order. In another it draws another mesh, or panics on a mismatch. `None` for parameters
    /// over [`MAX_PARAMS`](Self::MAX_PARAMS) bytes.
    #[doc(hidden)]
    pub fn from_parts(
        mesh: u32,
        shader: u32,
        opaque: bool,
        culls: bool,
        params: &[u8],
    ) -> Option<Self> {
        let mut copy = [0; Self::MAX_PARAMS];
        copy.get_mut(..params.len())?.copy_from_slice(params);
        Some(Self {
            mesh,
            shader,
            opaque,
            culls,
            params: copy,
            len: params.len() as u8,
        })
    }
}

/// A mesh's level of detail: whole triangles of its indices, drawn in place of the mesh's when it's
/// small enough on screen that their surface strays from it by at most a pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lod {
    /// Where its indices start in the mesh's.
    pub start: u32,
    /// Three per triangle.
    pub count: u32,
    /// How far its surface may stray from level 0's, in the mesh's units.
    pub error: f32,
}

/// The most levels of detail a mesh has, level 0 among them.
pub const MAX_LEVELS: usize = 8;

/// A mesh's levels of detail, finest first, and the sphere round its vertices that picks among
/// them.
///
/// The sphere also culls. Level 0 is the mesh itself, usually of error 0; errors never decrease.
/// Levels may share indices.
#[derive(Clone, Debug, PartialEq)]
pub struct Lods {
    /// Finest first, at most [`MAX_LEVELS`].
    pub levels: Vec<Lod>,
    /// The center of a sphere holding every vertex, in the mesh's space.
    pub center: Vec3,
    /// How near the camera the mesh can reach. Larger than the tightest keeps finer levels farther
    /// out and draws unseen meshes; smaller leaves out draws a view sees.
    pub radius: f32,
}

impl Lods {
    /// One level of all `indices`, in an infinite sphere: [`Meshes::add`] can't read vertex
    /// positions.
    pub(super) fn whole(indices: usize) -> Self {
        let level = Lod {
            start: 0,
            count: indices as u32,
            error: 0.0,
        };
        Self {
            levels: vec![level],
            center: Vec3::ZERO,
            radius: f32::INFINITY,
        }
    }
}

/// The meshes a renderer draws, the engine's quad (for sprites) first.
///
/// Handles come at once; [`Uploads::take`] takes the data, frames stream it within their budget,
/// and a mesh draws once there. Replacements and removals follow in order. A renderer draws one
/// `Meshes`' meshes only.
pub struct Meshes {
    /// Added, replaced or removed since last taken, in order.
    pub(crate) changes: Vec<Change>,
    count: u32,
    /// Their handles are never given out again.
    removed: HashSet<u32>,
    /// Each mesh's bounding sphere, center and radius in its own space, by handle. `None` without a
    /// finite sphere, or once removed.
    spheres: Vec<Option<(Vec3, f32)>>,
    /// The engine's grid mesh, added when first asked for.
    grid: Option<u32>,
}

/// A change [`Uploads::take`] carries to a renderer.
pub(crate) enum Change {
    /// A mesh added, or replaced.
    Added(Added),
    /// By index.
    Removed(u32),
}

pub(crate) struct Added {
    pub(super) index: u32,
    /// Whether it replaces a mesh a renderer has, rather than adding one after them.
    pub(super) replacement: bool,
    pub(super) name: String,
    pub(super) vertices: Vec<u8>,
    pub(super) vertex_size: usize,
    pub(super) indices: Vec<u32>,
    pub(super) lods: Lods,
    /// A model's mesh's meshlets, if any: what culls each, and each level's.
    pub(super) meshlets: Option<MeshletParts>,
    /// A skinned model's mesh's vertex count, which the skin pass moves.
    pub(super) skinned: Option<u32>,
    /// An unskinned model's mesh's positions, which the path tracer reads in place. Boxed to keep
    /// changes small.
    pub(super) positions: Option<Box<Positions>>,
}

/// A model mesh's packed positions, as acceleration structures read them in place: from its
/// buffer's first vertex, after the header, `count` of them, each at `offset + step * counts`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Positions {
    pub(crate) offset: Vec3,
    pub(crate) step: f32,
    pub(crate) count: u32,
}

/// What culls a mesh's meshlets, kept apart from its buffer: their bounds, and each level's
/// meshlets ([`Meshlets::levels`](crate::Meshlets::levels)).
pub(crate) struct MeshletParts {
    pub(super) bounds: Vec<MeshletBounds>,
    pub(super) levels: Vec<[u32; 2]>,
}

impl Added {
    /// Model mesh `model`, named `name`, of handle `index`.
    fn model(index: u32, replacement: bool, name: &str, model: &ModelMesh) -> Self {
        let meshlets = &model.meshlets;
        Self {
            index,
            replacement,
            name: name.to_owned(),
            vertices: model.bytes(),
            vertex_size: size_of::<PackedVertex>(),
            indices: model.indices.clone(),
            lods: model.lods.clone(),
            meshlets: (!meshlets.meshlets.is_empty()).then(|| MeshletParts {
                bounds: model.meshlet_bounds(),
                levels: meshlets.levels.clone(),
            }),
            skinned: (!model.skin.is_empty()).then(|| {
                u32::try_from(model.vertices.len()).expect("a mesh's vertices have 32-bit indices")
            }),
            positions: model.skin.is_empty().then(|| {
                Box::new(Positions {
                    offset: model.packing.offset,
                    step: model.packing.step,
                    count: u32::try_from(model.vertices.len())
                        .expect("a mesh's vertices have 32-bit indices"),
                })
            }),
        }
    }
}

impl Meshes {
    /// How many it has handed out (the quad and, once asked for, the grid among them): the next
    /// index.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// How many handed out aren't removed, for the engine's tests.
    #[doc(hidden)]
    pub fn kept(&self) -> u32 {
        self.count - self.removed.len() as u32
    }

    /// The sphere round `draw`'s mesh's vertices, center and radius in its space, readable before
    /// it streams. `None` if infinite (a mesh added without levels) or removed.
    pub fn sphere_of(&self, draw: &MeshDraw) -> Option<(Vec3, f32)> {
        self.spheres.get(draw.mesh as usize).copied().flatten()
    }

    /// Keeps `lods`' sphere for mesh `index`, unless removed.
    fn keep_sphere(&mut self, index: u32, lods: &Lods) {
        if self.removed.contains(&index) {
            return;
        }
        let sphere = lods
            .radius
            .is_finite()
            .then_some((lods.center, lods.radius));
        match self.spheres.get_mut(index as usize) {
            Some(kept) => *kept = sphere,
            None => {
                debug_assert_eq!(self.spheres.len(), index as usize, "handles come in order");
                self.spheres.push(sphere);
            }
        }
    }

    /// The engine's grid mesh, a unit square of four triangles from its center. Added when first
    /// asked for, so a renderer without grids uploads nothing for it, and games' handles come where
    /// they would have.
    pub(crate) fn grid(&mut self) -> Mesh<Vec2> {
        let index = match self.grid {
            Some(index) => index,
            None => {
                let mesh = (self.add("grid", &grid::SQUARE, &grid::FAN))
                    .expect("the square is four whole triangles of its own corners");
                *self.grid.insert(mesh.index)
            }
        };
        Mesh::new(index)
    }

    /// The engine's meshes added so far when first asked for, each with its name and index, as
    /// [`Shaders::engine`](crate::Shaders::engine) gives the engine's shaders.
    pub fn engine(&self) -> impl Iterator<Item = (&'static str, u32)> + use<> {
        self.grid.map(|index| ("grid", index)).into_iter()
    }

    /// Adds the engine's mesh `name` (as [`engine`](Self::engine) names it) as a draw first asking
    /// for it would, unless it's added already. Gives its index; `None` for no such name.
    pub fn add_engine(&mut self, name: &str) -> Option<u32> {
        (name == "grid").then(|| self.grid().index)
    }

    /// Adds a mesh of triangles, three of `indices` each, indexing `vertices`; `name` names it in
    /// errors and captures. Shaders for `V` draw it ([`Shaders::add`](crate::Shaders::add)). It has
    /// one level of detail and no sphere, as it can't read positions, so it's never culled.
    ///
    /// Fails with `renderer.bad_mesh` for no triangles, partial triangles, or an index past
    /// `vertices`.
    pub fn add<V: NoUninit>(
        &mut self,
        name: &str,
        vertices: &[V],
        indices: &[u32],
    ) -> Result<Mesh<V>> {
        check(name, vertices, indices)?;
        Ok(self.push(name, vertices, indices, Lods::whole(indices.len())))
    }

    /// Adds a mesh as [`add`](Self::add) does, with `lods`' levels of detail among `indices`, in
    /// one GPU buffer. Each draw uses the coarsest level whose error spans at most a pixel, or
    /// level 0 if the sphere reaches the camera's plane. An opaque draw whose sphere is outside a
    /// view isn't drawn there, so a shader moving vertices out of the sphere may lose them.
    ///
    /// Fails with `renderer.bad_mesh` as `add` does, or if `lods` has no levels or over
    /// [`MAX_LEVELS`]; a level empty, of partial triangles or past `indices`; an error negative,
    /// not finite or below the last's; a center not finite; or a radius negative or not finite.
    pub fn add_lods<V: NoUninit>(
        &mut self,
        name: &str,
        vertices: &[V],
        indices: &[u32],
        lods: Lods,
    ) -> Result<Mesh<V>> {
        check(name, vertices, indices)?;
        check_lods(name, indices, &lods)?;
        Ok(self.push(name, vertices, indices, lods))
    }

    /// Adds a model's mesh as [`add_lods`](Self::add_lods) does, its vertices packed for the model
    /// shaders. Views cull and draw it by its meshlets, if any, where it's opaque ([`ModelMesh`]);
    /// elsewhere by its indices.
    ///
    /// Fails with `renderer.bad_mesh` as [`ModelMesh::check`] does.
    pub fn add_model(&mut self, name: &str, mesh: &ModelMesh) -> Result<Mesh<PackedVertex>> {
        mesh.check(name)?;
        let index = self.count;
        self.count += 1;
        self.keep_sphere(index, &mesh.lods);
        self.changes
            .push(Change::Added(Added::model(index, false, name, mesh)));
        Ok(Mesh::new(index))
    }

    /// Hands out the next handle, for a mesh `check` passed.
    fn push<V: NoUninit>(
        &mut self,
        name: &str,
        vertices: &[V],
        indices: &[u32],
        lods: Lods,
    ) -> Mesh<V> {
        let index = self.count;
        self.count += 1;
        self.keep_sphere(index, &lods);
        self.changes.push(Change::Added(Added {
            index,
            replacement: false,
            name: name.to_owned(),
            vertices: bytemuck::cast_slice(vertices).to_vec(),
            vertex_size: size_of::<V>(),
            indices: indices.to_vec(),
            lods,
            meshlets: None,
            skinned: None,
            positions: None,
        }));
        Mesh::new(index)
    }

    /// Puts `vertices` and `indices` (as [`add`](Self::add) takes them) in place of `mesh`'s,
    /// keeping its handle and one level of detail. Draws show the old triangles until the new ones
    /// stream, which replace any still waiting. A removed mesh stays removed.
    ///
    /// Fails with `renderer.bad_mesh` as `add` does, leaving the mesh as it was.
    ///
    /// # Panics
    ///
    /// - If `mesh` isn't one of these meshes (another `Meshes`' handles can be past them).
    pub fn replace<V: NoUninit>(
        &mut self,
        name: &str,
        mesh: Mesh<V>,
        vertices: &[V],
        indices: &[u32],
    ) -> Result<()> {
        self.replace_with(name, mesh, vertices, indices, None)
    }

    /// [`replace`](Self::replace), with levels of detail as [`add_lods`](Self::add_lods) takes
    /// them.
    ///
    /// Fails with `renderer.bad_mesh` as `add_lods` does, leaving the mesh as it was.
    ///
    /// # Panics
    ///
    /// - If `mesh` isn't one of these meshes (another `Meshes`' handles can be past them).
    pub fn replace_lods<V: NoUninit>(
        &mut self,
        name: &str,
        mesh: Mesh<V>,
        vertices: &[V],
        indices: &[u32],
        lods: Lods,
    ) -> Result<()> {
        self.replace_with(name, mesh, vertices, indices, Some(lods))
    }

    /// Replaces `mesh`'s triangles with `lods`' levels, or one of all `indices`
    /// ([`replace_lods`](Self::replace_lods), [`replace`](Self::replace)).
    fn replace_with<V: NoUninit>(
        &mut self,
        name: &str,
        mesh: Mesh<V>,
        vertices: &[V],
        indices: &[u32],
        lods: Option<Lods>,
    ) -> Result<()> {
        assert!(
            mesh.index < self.count,
            "mesh {name:?} replaces one of another `Meshes`"
        );
        check(name, vertices, indices)?;
        if let Some(lods) = &lods {
            check_lods(name, indices, lods)?;
        }
        let lods = lods.unwrap_or_else(|| Lods::whole(indices.len()));
        self.keep_sphere(mesh.index, &lods);
        self.replace_added(Added {
            index: mesh.index,
            replacement: true,
            name: name.to_owned(),
            vertices: bytemuck::cast_slice(vertices).to_vec(),
            vertex_size: size_of::<V>(),
            indices: indices.to_vec(),
            lods,
            meshlets: None,
            skinned: None,
            positions: None,
        });
        Ok(())
    }

    /// [`replace_lods`](Self::replace_lods), with a model's mesh as [`add_model`](Self::add_model)
    /// takes it.
    ///
    /// Fails with `renderer.bad_mesh` as `add_model` does, leaving the mesh as it was.
    ///
    /// # Panics
    ///
    /// - If `mesh` isn't one of these meshes (another `Meshes`' handles can be past them).
    pub fn replace_model(
        &mut self,
        name: &str,
        mesh: Mesh<PackedVertex>,
        model: &ModelMesh,
    ) -> Result<()> {
        assert!(
            mesh.index < self.count,
            "mesh {name:?} replaces one of another `Meshes`"
        );
        model.check(name)?;
        self.keep_sphere(mesh.index, &model.lods);
        self.replace_added(Added::model(mesh.index, true, name, model));
        Ok(())
    }

    /// Queues `added`, a replacement, in place of any of the mesh still waiting, unless it was
    /// removed.
    fn replace_added(&mut self, added: Added) {
        if self.removed.contains(&added.index) {
            return;
        }
        let waiting = (self.changes.iter_mut()).find_map(|change| match change {
            Change::Added(waiting) if waiting.index == added.index => Some(waiting),
            _ => None,
        });
        match waiting {
            Some(waiting) => {
                waiting.vertices = added.vertices;
                waiting.indices = added.indices;
                waiting.lods = added.lods;
                waiting.meshlets = added.meshlets;
            }
            None => self.changes.push(Change::Added(added)),
        }
    }

    /// Removes `mesh`: its draws draw nothing from the frame that takes the removal, unstreamed
    /// data is dropped, and its buffers go once the GPU is done with them. Its handle is never
    /// reused, so a stale one (in a world or snapshot) never draws another mesh. Returns whether it
    /// wasn't already removed.
    ///
    /// # Panics
    ///
    /// - If `mesh` isn't one of these meshes (another `Meshes`' handles can be past them).
    pub fn remove<V>(&mut self, mesh: Mesh<V>) -> bool {
        self.remove_index(mesh.index)
    }

    /// [`remove`](Self::remove) by index, whatever the vertex type, for the engine's restores,
    /// which hold meshes by index, as snapshots do.
    ///
    /// # Panics
    ///
    /// - If `index` is past the meshes handed out.
    #[doc(hidden)]
    pub fn remove_index(&mut self, index: u32) -> bool {
        assert!(
            index < self.count,
            "mesh {index} is one of another `Meshes`"
        );
        let removed = self.removed.insert(index);
        if removed {
            self.spheres[index as usize] = None;
            self.changes.push(Change::Removed(index));
        }
        removed
    }
}

/// Checks that `indices` are whole triangles of `vertices` ([`Meshes::add`]).
pub(crate) fn check<V>(name: &str, vertices: &[V], indices: &[u32]) -> Result<()> {
    // Vulkan's buffers hold at least a byte.
    const { assert!(size_of::<V>() > 0, "a vertex type has a size") };
    let bad = |problem: String| bad_mesh(name, problem);
    if indices.is_empty() {
        return Err(bad("has no triangles".to_owned()));
    }
    if !indices.len().is_multiple_of(3) {
        return Err(bad(format!(
            "has {} indices, not three for each triangle",
            indices.len()
        )));
    }
    if let Some(index) = indices.iter().find(|&&i| i as usize >= vertices.len()) {
        return Err(bad(format!(
            "has index {index}, past its {} vertices",
            vertices.len()
        )));
    }
    Ok(())
}

/// Checks `lods` against `indices` as [`Meshes::add_lods`] says.
pub(crate) fn check_lods(name: &str, indices: &[u32], lods: &Lods) -> Result<()> {
    let bad = |problem: String| bad_mesh(name, problem);
    if lods.levels.is_empty() {
        return Err(bad("has no levels of detail".to_owned()));
    }
    if lods.levels.len() > MAX_LEVELS {
        return Err(bad(format!(
            "has {} levels of detail, past the {MAX_LEVELS} a mesh has at most",
            lods.levels.len()
        )));
    }
    // 0 for level 0, since an error may not be negative.
    let mut finer = 0.0;
    for (i, lod) in lods.levels.iter().enumerate() {
        let Lod {
            start,
            count,
            error,
        } = *lod;
        let end = u64::from(start) + u64::from(count);
        if end > indices.len() as u64 {
            return Err(bad(format!(
                "has level {i} from index {start} to {end}, past its {} indices",
                indices.len()
            )));
        }
        if count == 0 {
            return Err(bad(format!("has level {i} without triangles")));
        }
        if !count.is_multiple_of(3) {
            return Err(bad(format!(
                "has level {i} with {count} indices, not three for each triangle"
            )));
        }
        if !(error.is_finite() && error >= 0.0) {
            return Err(bad(format!(
                "has level {i} with error {error}, not a finite distance of 0 or more"
            )));
        }
        if error < finer {
            return Err(bad(format!(
                "has level {i} with error {error}, less than the {finer} of the level before"
            )));
        }
        finer = error;
    }
    if !lods.center.is_finite() {
        return Err(bad(format!("has a sphere centered at {}", lods.center)));
    }
    if !(lods.radius.is_finite() && lods.radius >= 0.0) {
        return Err(bad(format!(
            "has a sphere of radius {}, not a finite length of 0 or more",
            lods.radius
        )));
    }
    Ok(())
}

/// The error for mesh `name`, with `problem` a phrase following its name.
pub(crate) fn bad_mesh(name: &str, problem: String) -> Error {
    Error::new("renderer.bad_mesh", format!("mesh {name:?} {problem}"))
}

impl Default for Meshes {
    fn default() -> Self {
        let mut meshes = Self {
            changes: Vec::new(),
            count: 0,
            removed: HashSet::new(),
            spheres: Vec::new(),
            grid: None,
        };
        let quad = meshes
            .add("quad", &sprite::CORNERS, &sprite::INDICES)
            .expect("the quad is whole triangles of its own corners");
        assert_eq!(quad.index, sprite::QUAD.index, "the quad comes first");
        meshes
    }
}
