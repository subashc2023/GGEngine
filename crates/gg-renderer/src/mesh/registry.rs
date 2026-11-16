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

/// A mesh that [`Meshes::add`] handed out, of vertex type `V`.
pub struct Mesh<V> {
    pub(crate) index: u32,
    vertex: PhantomData<fn() -> V>,
}

// By hand, so that they don't need `V` to be `Clone` or `Debug` too.
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

    /// Its index, the order [`Meshes::add`] handed it out in, from 0, the engine's quad's: what a
    /// [`MeshDraw`]'s parts hold it by.
    pub fn index(self) -> u32 {
        self.index
    }
}

impl<V> fmt::Debug for Mesh<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Mesh").field(&self.index).finish()
    }
}

/// A mesh to draw with a shader and the parameters it takes, as [`Draws::push`] draws one, without
/// their types: they're checked against each other as it's made, then dropped, so that one type
/// holds a draw of any mesh, as an entity's component does. Its parameters are copied into it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshDraw {
    pub(crate) mesh: u32,
    pub(crate) shader: u32,
    /// Whether its shader writes depth.
    pub(super) opaque: bool,
    /// Whether it draws only its mesh's fronts, as a material that isn't double-sided does.
    pub(crate) culls: bool,
    pub(super) params: [u8; MeshDraw::MAX_PARAMS],
    /// How many bytes of `params` are the parameters'.
    pub(super) len: u8,
}

impl MeshDraw {
    /// The most bytes of parameters it holds, until materials keep parameters on the GPU; a shader
    /// that takes more draws through [`Draws::push`].
    pub const MAX_PARAMS: usize = 64;

    /// A draw of `mesh` with `shader`, which reads its vertex type, giving the shader `params`.
    /// Parameters of more than [`MAX_PARAMS`](Self::MAX_PARAMS) bytes don't compile.
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

    /// The same draw, of `mesh` in place of its own: of the same vertex type as its own, as a
    /// skinned model's instance's own copy of its model's mesh is, or its draws panic, as they
    /// check the mesh's vertices against the shader's.
    pub fn with_mesh<V>(self, mesh: Mesh<V>) -> Self {
        Self {
            mesh: mesh.index,
            ..self
        }
    }

    /// What it batches by, if its shader is opaque: the same for every draw of its mesh with its
    /// shader, and ordered by shader, then mesh. An opaque shader writes depth, which hides what's
    /// behind it whatever order they draw in, so opaque draws can go in this one, which makes each
    /// run of them one instanced draw.
    pub fn batch(&self) -> Option<u64> {
        self.opaque
            .then(|| (u64::from(self.shader) << 32) | u64::from(self.mesh))
    }

    /// Its parameters, as bytes.
    pub(crate) fn params(&self) -> &[u8] {
        &self.params[..usize::from(self.len)]
    }

    /// Its parts as a snapshot keeps them, which [`from_parts`](Self::from_parts) makes it again
    /// from: its mesh's index and its shader's, whether its shader is opaque, whether it culls,
    /// and its parameters' bytes.
    #[doc(hidden)]
    pub fn parts(&self) -> (u32, u32, bool, bool, &[u8]) {
        let parts = (self.mesh, self.shader, self.opaque, self.culls);
        (parts.0, parts.1, parts.2, parts.3, self.params())
    }

    /// A draw of the parts [`parts`](Self::parts) gave, for a renderer handed the same meshes and
    /// shaders, in the same order, as the one it was made for: in another, it draws another mesh,
    /// or panics as it draws, if its vertices or its parameters aren't its shader's. `None` for
    /// parameters of more than [`MAX_PARAMS`](Self::MAX_PARAMS) bytes.
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

/// A level of detail of a mesh: some of its indices, whole triangles of them, that a draw uses in
/// place of the mesh's own when the mesh is small enough on screen that their surface strays from
/// it by no more than a pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lod {
    /// Where its indices start in the mesh's.
    pub start: u32,
    /// How many indices it has, three for each triangle.
    pub count: u32,
    /// How far its surface may stray from the mesh's own, level 0's, in the mesh's own units.
    pub error: f32,
}

/// The most levels of detail a mesh has, its own among them.
pub const MAX_LEVELS: usize = 8;

/// A mesh's levels of detail, and the sphere round its vertices that decides which a draw uses,
/// and whether a view sees it. The levels go finest first, so level 0 is the mesh itself, usually
/// with an error of 0, and each has at least the error of the one before. They may share indices,
/// as a chain of levels simplified one from another does.
#[derive(Clone, Debug, PartialEq)]
pub struct Lods {
    /// The levels, finest first, [`MAX_LEVELS`] at most.
    pub levels: Vec<Lod>,
    /// The center of a sphere that holds every vertex, in the mesh's own space.
    pub center: Vec3,
    /// Its radius: how near the camera the mesh can reach, where it is largest on screen. One
    /// larger than the tightest only keeps finer levels in use at greater distances, and draws
    /// where no view sees it; one smaller leaves out draws that a view sees.
    pub radius: f32,
}

impl Lods {
    /// The levels of a mesh that has one, all of its `indices`, in a sphere of infinite radius,
    /// which holds whatever vertices it has: [`Meshes::add`] can't read its vertices' positions.
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

/// The meshes a renderer draws, the first of them the engine's quad, which sprites are drawn on.
/// It hands out each one's handle at once, and keeps its vertices and indices until
/// [`Uploads::take`] takes them, for frames to stream to the GPU within their budget, drawing the
/// mesh once they're there; a mesh replaced by others the same way, which draw in place of the old
/// once they're there; and a mesh removed goes with them, in order. A renderer draws the meshes of
/// one `Meshes` only.
pub struct Meshes {
    /// Those added, replaced or removed since they were last taken, in order.
    pub(crate) changes: Vec<Change>,
    /// How many have been added.
    count: u32,
    /// Those removed, whose handles are never given out again.
    removed: HashSet<u32>,
    /// Each handed-out mesh's bounding sphere, its center and radius in the mesh's own space, by
    /// its handle; `None` for one without a finite sphere, and for one removed.
    spheres: Vec<Option<(Vec3, f32)>>,
    /// The handle of the engine's grid mesh, which is added the first time it's asked for, once
    /// it has been.
    grid: Option<u32>,
}

/// A change to a renderer's meshes, which [`Uploads::take`] carries to it.
pub(crate) enum Change {
    /// A mesh added, or vertices and indices in place of one's.
    Added(Added),
    /// A mesh removed, by its index.
    Removed(u32),
}

pub(crate) struct Added {
    pub(super) index: u32,
    /// Whether it's vertices and indices in place of a mesh's a renderer has, rather than a mesh
    /// added, which comes after those a renderer has.
    pub(super) replacement: bool,
    pub(super) name: String,
    pub(super) vertices: Vec<u8>,
    pub(super) vertex_size: usize,
    pub(super) indices: Vec<u32>,
    pub(super) lods: Lods,
    /// A model's mesh's meshlets, if it has them: what culls each, and each level's.
    pub(super) meshlets: Option<MeshletParts>,
    /// For a skinned model's mesh, how many vertices it has, which the skin pass moves.
    pub(super) skinned: Option<u32>,
}

/// What culls a mesh's meshlets, as the renderer keeps it apart from the mesh's buffer: each
/// meshlet's bounds, and each level's meshlets, as [`Meshlets::levels`](crate::Meshlets::levels)
/// has them.
pub(crate) struct MeshletParts {
    pub(super) bounds: Vec<MeshletBounds>,
    pub(super) levels: Vec<[u32; 2]>,
}

impl Added {
    /// Model's mesh `model`, named `name`, of handle `index`, as a replacement or as one added.
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
        }
    }
}

impl Meshes {
    /// How many it has handed out, the engine's quad among them, and its grid mesh once it was
    /// first asked for: the index of the next.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// How many of those it has handed out aren't removed: for the engine's tests of what it
    /// keeps.
    #[doc(hidden)]
    pub fn kept(&self) -> u32 {
        self.count - self.removed.len() as u32
    }

    /// The sphere round the vertices of the mesh `draw` draws, as its levels of detail hold it: its
    /// center and radius in the mesh's own space. `None` for a mesh whose sphere is infinite, as one
    /// added without levels of detail is, whose vertex positions can't be read, and for one removed.
    /// Readable at once, before a frame has streamed the mesh to the GPU.
    pub fn sphere_of(&self, draw: &MeshDraw) -> Option<(Vec3, f32)> {
        self.spheres.get(draw.mesh as usize).copied().flatten()
    }

    /// Keeps `lods`' sphere as the sphere of the mesh of index `index`, unless it was removed.
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

    /// The engine's grid mesh, a unit square as four triangles from its center, added as a game's
    /// meshes are the first time it's asked for: a renderer draws grids with it, but uploads
    /// nothing for it without them, and games' handles come where they would have.
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

    /// Adds a mesh of triangles, three of `indices` each, which index `vertices`. `name` names it
    /// in errors and GPU captures. It draws with shaders for `V`, which read its vertices as
    /// [`Shaders::add`](crate::Shaders::add) says. It has one level of detail, all its indices,
    /// and no sphere round its vertices, whose positions it can't read: every draw of it draws,
    /// wherever a view sees.
    ///
    /// Fails with `renderer.bad_mesh` if it has no triangles, indices left over from whole
    /// triangles, or an index past its vertices.
    pub fn add<V: NoUninit>(
        &mut self,
        name: &str,
        vertices: &[V],
        indices: &[u32],
    ) -> Result<Mesh<V>> {
        check(name, vertices, indices)?;
        Ok(self.push(name, vertices, indices, Lods::whole(indices.len())))
    }

    /// Adds a mesh as [`add`](Self::add) does, with the levels of detail `lods` describes, whose
    /// triangles are among `indices`: each draw of it uses the coarsest level whose error spans no
    /// more than a pixel on screen, as the draw sees it, or level 0 if none does, or if the sphere
    /// reaches the camera's plane. Every level indexes the same vertices, and all of them are
    /// kept in one buffer on the GPU. A draw with an opaque shader whose sphere, as its transform
    /// places it, is wholly outside what a view sees, the game's view or a shadow's cascade, isn't
    /// drawn there, so a shader that moves the mesh's vertices outside the sphere may lose them.
    ///
    /// Fails with `renderer.bad_mesh` as `add` does over all of `indices`, and if `lods` has no
    /// levels, or more than [`MAX_LEVELS`]; has a level that is empty, isn't whole triangles or
    /// reaches past `indices`; has an error that is negative, isn't finite, or is less than the
    /// level before's; has a center that isn't finite; or a radius that is negative or isn't
    /// finite.
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

    /// Adds a model's mesh, `mesh`, as [`add_lods`](Self::add_lods) adds one, its vertices packed,
    /// which the model shaders draw, and its meshlets, if it has any, which views cull and draw it
    /// by where it's opaque, as [`ModelMesh`] says; its indices draw it elsewhere.
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

    /// Hands out the next handle for a mesh `check` has passed.
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
        }));
        Mesh::new(index)
    }

    /// Puts other triangles in place of `mesh`'s, which keeps its handle: `indices` and `vertices`
    /// as [`add`](Self::add) takes them, as many as they are, so that every draw of it draws them
    /// once frames have streamed them to the GPU, drawing the triangles it had until then. It takes
    /// the place of any of that mesh's vertices and indices still waiting for a frame, as it was
    /// added or replaced. `name` names it in errors. The mesh has one level of detail again, all
    /// its indices. A mesh removed stays removed: a replacement of it is dropped.
    ///
    /// Fails with `renderer.bad_mesh` as `add` does, leaving the mesh as it was.
    ///
    /// # Panics
    ///
    /// If `mesh` isn't one of these meshes, as another `Meshes`' handles can be past them.
    pub fn replace<V: NoUninit>(
        &mut self,
        name: &str,
        mesh: Mesh<V>,
        vertices: &[V],
        indices: &[u32],
    ) -> Result<()> {
        self.replace_with(name, mesh, vertices, indices, None)
    }

    /// Puts other triangles in place of `mesh`'s, as [`replace`](Self::replace) does, with the
    /// levels of detail `lods` describes, as [`add_lods`](Self::add_lods) takes them.
    ///
    /// Fails with `renderer.bad_mesh` as `add_lods` does, leaving the mesh as it was.
    ///
    /// # Panics
    ///
    /// If `mesh` isn't one of these meshes, as another `Meshes`' handles can be past them.
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

    /// Puts other triangles in place of `mesh`'s, of the levels `lods` describes, or of one, all of
    /// `indices`, as [`replace_lods`](Self::replace_lods) and [`replace`](Self::replace) say.
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
        });
        Ok(())
    }

    /// Puts a model's mesh, `model`, in place of `mesh`'s, as [`replace_lods`](Self::replace_lods)
    /// does, as [`add_model`](Self::add_model) takes one.
    ///
    /// Fails with `renderer.bad_mesh` as `add_model` does, leaving the mesh as it was.
    ///
    /// # Panics
    ///
    /// If `mesh` isn't one of these meshes, as another `Meshes`' handles can be past them.
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

    /// Queues `added`, a mesh's replacement, in place of any of it still waiting for a frame, unless
    /// the mesh was removed.
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

    /// Removes `mesh`: its buffers go from the GPU once the frames and copies that may use them
    /// have finished, whatever of it still waits to be streamed is dropped, and every draw of it
    /// draws nothing from the frame that takes the removal on. Its handle is never given out
    /// again, so a stale one, in a world or a snapshot, never draws another mesh. Removing one
    /// removed already does nothing. Returns whether it removed it, rather than finding it removed
    /// already.
    ///
    /// # Panics
    ///
    /// If `mesh` isn't one of these meshes, as another `Meshes`' handles can be past them.
    pub fn remove<V>(&mut self, mesh: Mesh<V>) -> bool {
        self.remove_index(mesh.index)
    }

    /// Removes the mesh of index `index`, as [`remove`](Self::remove) does, whatever its vertex
    /// type: for the engine's restores, which hold a mesh by its index alone, as a snapshot does.
    ///
    /// # Panics
    ///
    /// If `index` is past the meshes handed out.
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

/// Checks that `indices` are whole triangles of `vertices`, mesh `name`'s, as [`Meshes::add`]
/// says.
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

/// Checks that `lods`' levels are whole triangles among `indices`, mesh `name`'s, and the rest of
/// what [`Meshes::add_lods`] says.
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

/// The error for mesh `name`, which has the problem `problem` says, in a phrase that follows its
/// name.
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
