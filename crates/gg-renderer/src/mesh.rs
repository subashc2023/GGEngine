//! Meshes: the handles that the engine draws with, and the vertices and indices behind them in
//! GPU memory, drawn with their shaders and textures, games' and the engine's, in the game's view,
//! as its camera sees them.

use std::collections::{HashSet, VecDeque};
use std::ffi::{CStr, CString};
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_gpu::{
    Blend, ComputePipeline, Device, Format, Frame, GpuBuffer, GraphicsDesc, MemoryBudget, Pass,
    Pipeline, Texture as GpuTexture, TextureFormat,
};
use tracing::warn;

use crate::draws::Run;
use crate::packed::{MeshletBounds, ModelMesh, PackedVertex};
use crate::particles::ParticleDraws;
use crate::shader::{self, Shader};
use crate::texture::{self, Map, Texels};
use crate::{DrawCounts, Draws, Streaming, Uploads, grid, model, shadow, sprite};

mod cull;
mod scene;
mod visibility;

#[cfg(test)]
pub(crate) use cull::SPIRV as CULL_SPIRV;
pub(crate) use cull::{CullPlan, CullView, FIRST, counts};
#[cfg(test)]
pub(crate) use cull::{GATHER_SPIRV, PYRAMID_SPIRV};
pub(crate) use scene::Change as SceneChange;
pub use scene::Scene;
#[cfg(test)]
pub(crate) use visibility::SPIRV as VISIBILITY_SPIRV;
pub(crate) use visibility::Shaded;

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
    opaque: bool,
    /// Whether it draws only its mesh's fronts, as a material that isn't double-sided does.
    pub(crate) culls: bool,
    params: [u8; MeshDraw::MAX_PARAMS],
    /// How many bytes of `params` are the parameters'.
    len: u8,
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
    fn whole(indices: usize) -> Self {
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
    index: u32,
    /// Whether it's vertices and indices in place of a mesh's a renderer has, rather than a mesh
    /// added, which comes after those a renderer has.
    replacement: bool,
    name: String,
    vertices: Vec<u8>,
    vertex_size: usize,
    indices: Vec<u32>,
    lods: Lods,
    /// A model's mesh's meshlets, if it has them: what culls each, and each level's.
    meshlets: Option<MeshletParts>,
    /// For a skinned model's mesh, how many vertices it has, which the skin pass moves.
    skinned: Option<u32>,
}

/// What culls a mesh's meshlets, as the renderer keeps it apart from the mesh's buffer: each
/// meshlet's bounds, and each level's meshlets, as [`Meshlets::levels`](crate::Meshlets::levels)
/// has them.
pub(crate) struct MeshletParts {
    bounds: Vec<MeshletBounds>,
    levels: Vec<[u32; 2]>,
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
            grid: None,
        };
        let quad = meshes
            .add("quad", &sprite::CORNERS, &sprite::INDICES)
            .expect("the quad is whole triangles of its own corners");
        assert_eq!(quad.index, sprite::QUAD.index, "the quad comes first");
        meshes
    }
}

/// Draws meshes with their shaders into the game's view, colors of high dynamic range of
/// [`Format::HDR`], writing their IDs beside them and testing its depth image, streams the
/// meshes' vertices and indices and the textures the shaders sample to the GPU within a budget of
/// bytes each frame, and keeps them there.
pub(crate) struct MeshPass {
    device: Arc<Device>,
    /// Each shader's pipeline, by its handle; `None` for one removed.
    shaders: Vec<Option<GpuShader>>,
    /// Each mesh's buffers, by its handle, once they're drawable.
    meshes: Vec<Held<GpuMesh>>,
    /// Each texture, by its handle, which is its slot in the bindless sets too.
    textures: Vec<TextureSlot>,
    /// What the renderer's slots show until their textures' images are there: dropped after the
    /// textures, whose slots show them.
    stand_ins: StandIns,
    /// What's being streamed to the GPU, in the order it was added, each until it's drawable or
    /// removed; it keeps its memory, so that a frame with nothing to stream allocates nothing.
    streaming: VecDeque<Item>,
    /// The materials models' draws index.
    materials: MaterialTable,
    /// The most bytes a frame copies, as [`Settings::upload_budget`](crate::Settings::upload_budget)
    /// has it, but for the smallest whole part.
    budget: u64,
    /// What culls the frame's opaque runs, and what it culled.
    culling: cull::Culling,
    /// The scene's instances, as the GPU keeps them.
    scene: scene::Kept,
    /// What culls the meshlets of the meshes drawn by them.
    meshlet_table: MeshletTable,
    /// What shades the game's view from its visibility buffer.
    shading: ComputePipeline,
    /// Where this frame's skin pass moved each skinned mesh's vertices, by the mesh's handle, which
    /// its draws read in place of its own; 0 for a mesh it didn't move. Kept from frame to frame,
    /// each frame setting again those it sets, so that it allocates nothing once it's grown.
    skinned: Vec<u64>,
    /// The meshes `skinned` holds an address for.
    skinned_set: Vec<u32>,
}

/// Where a frame's draws' instances are, as [`MeshPass::upload_draws`] copied them into the frame:
/// the addresses of their transforms, their IDs and their parameters, 0 for none.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Uploaded {
    transforms: u64,
    ids: u64,
    params: u64,
}

struct GpuShader {
    pipeline: Pipeline,
    /// What draws its depth alone, if it writes depth: its vertex shader, and `shadow_main` if its
    /// shadows are cut out ([`shader::Added::shadow`]).
    depth_only: Option<DepthOnly>,
    /// What draws the meshlets a view keeps of a mesh, and their depth alone, for a model shader
    /// that views draw meshes with meshlets by: through mesh shaders where the device has them, and
    /// through vertex shaders elsewhere ([`shader::Added::meshlets`]).
    meshlets: Option<(Pipeline, Option<DepthOnly>)>,
    /// What draws them into the visibility buffer, for such a shader, likewise.
    visibility: Option<Pipeline>,
    /// Its name, which captures label its runs of draws with.
    label: CString,
    /// The size of the vertices it reads.
    vertex_size: usize,
    /// The size of the parameters it takes.
    params_size: usize,
    /// Whether the pipeline writes depth, as [`shader::writes_depth`] says.
    writes_depth: bool,
}

/// What draws a shader's depth alone, by its geometry: into a shadow map, its depth clamped to the
/// map's near side and set back by its slope; and into the depth the view's ambient occlusion
/// takes, as the view draws it.
struct DepthOnly {
    shadow: Pipeline,
    view: Pipeline,
}

struct GpuMesh {
    vertices: GpuBuffer,
    /// Every level's indices.
    indices: GpuBuffer,
    /// Where each level is in `indices`, and the sphere that picks among them: a mesh added
    /// without levels has one, all its indices.
    lods: Lods,
    vertex_size: usize,
    /// Where its meshlets are in the table, for a model's mesh that has them.
    meshlets: Option<PlacedMeshlets>,
    /// For a skinned model's mesh, how many vertices it has, which the skin pass moves.
    skinned: Option<u32>,
}

/// What the pass holds for a mesh's handle: nothing yet, while what was added streams, what it
/// draws with, or nothing again, once removed.
enum Held<T> {
    Waiting,
    Ready(T),
    Removed,
}

impl<T> Held<T> {
    fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(ready) => Some(ready),
            Self::Waiting | Self::Removed => None,
        }
    }
}

/// A texture's slot: the texture in it, what its texels stand for, which says which stand-in the
/// slot shows without an image of its own, whether it was removed, which leaves it showing the
/// stand-in for good, as stale handles may still sample it, and how many times it has come to show
/// another image, its own or the stand-in, which tells what it shows apart from what it showed.
struct TextureSlot {
    texture: GpuTexture,
    kind: texture::Kind,
    removed: bool,
    shown: u64,
}

/// The textures the renderer's slots show until their own images are there: a pixel of white for
/// colors, so that a sprite shows its tint, and for a surface's map, so that a material shows its
/// factors; a pixel of zeros for data, so that text draws nothing rather than boxes; and a normal
/// pointing straight out for a normal map, so that a surface shows as its vertices shape it.
/// Written in the first frame that begins, before any slot shows them.
struct StandIns {
    white: GpuTexture,
    zeros: GpuTexture,
    flat: GpuTexture,
    written: bool,
}

impl StandIns {
    fn new(device: &Arc<Device>) -> Result<Self> {
        let white = TextureFormat::Rgba8Srgb;
        let data = TextureFormat::Rgba8Unorm;
        Ok(Self {
            white: GpuTexture::new(device, 1, 1, white, false, "white stand-in")?,
            zeros: GpuTexture::new(device, 1, 1, data, false, "zero stand-in")?,
            flat: GpuTexture::new(device, 1, 1, data, false, "flat normal stand-in")?,
            written: false,
        })
    }

    /// The stand-in for a texture whose texels stand for `kind`.
    fn of(&self, kind: texture::Kind) -> &GpuTexture {
        match kind {
            texture::Kind::Color | texture::Kind::Map(Map::Surface) => &self.white,
            texture::Kind::Data => &self.zeros,
            texture::Kind::Map(Map::Normal) => &self.flat,
        }
    }

    /// Writes them, in the first frame.
    fn write(&mut self, frame: &mut Frame<'_>) -> Result<()> {
        if self.written {
            return Ok(());
        }
        frame.write_texture(&mut self.white, [0, 0], [1, 1], &[255; 4])?;
        frame.write_texture(&mut self.zeros, [0, 0], [1, 1], &[0; 4])?;
        // X and Y of 0, which a byte holds as nearly as 128 does.
        frame.write_texture(&mut self.flat, [0, 0], [1, 1], &[128, 128, 255, 255])?;
        self.written = true;
        Ok(())
    }
}

/// The table of materials, by their handles, in the GPU's own memory: written as materials are added
/// and replaced, in the frame's own commands, before its passes, so that every draw from that
/// frame on reads what was written; and made again, twice the size, as more are added than it
/// holds, the old one retiring until the GPU is done with it. What it holds is kept on the CPU
/// too, for a table made again to start from.
struct MaterialTable {
    /// The table, and how many materials it holds.
    table: Option<(GpuBuffer, usize)>,
    entries: Vec<model::Entry>,
}

impl MaterialTable {
    /// The fewest materials a table holds.
    const LEAST: usize = 64;

    /// Writes `changes`, taking them, each a material's index and its entry, into the table, which
    /// it makes, or makes again larger, as they need: one copy of every entry from the first that
    /// changed to the last, or of them all into a table made again.
    ///
    /// # Panics
    ///
    /// If a material's index is past those before it, as they are where they came from another
    /// `Materials` than those before them.
    fn write(
        &mut self,
        device: &Arc<Device>,
        frame: &mut Frame<'_>,
        changes: &mut Vec<(u32, model::Entry)>,
    ) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        // The first entry that changed, and the one past the last.
        let (mut first, mut end) = (usize::MAX, 0);
        for (index, entry) in changes.drain(..) {
            let index = index as usize;
            match index.cmp(&self.entries.len()) {
                std::cmp::Ordering::Less => self.entries[index] = entry,
                std::cmp::Ordering::Equal => self.entries.push(entry),
                std::cmp::Ordering::Greater => {
                    panic!("a renderer draws with the materials of one `Materials`")
                }
            }
            (first, end) = (first.min(index), end.max(index + 1));
        }
        let size = size_of::<model::Entry>();
        let len = self.entries.len();
        if self.table.as_ref().is_none_or(|&(_, room)| room < len) {
            let room = len.next_power_of_two().max(Self::LEAST);
            // The table that's there, if any, retires until the GPU is done with the frames that
            // read it.
            let table = GpuBuffer::new(device, (room * size) as u64, "materials")?;
            self.table = Some((table, room));
            (first, end) = (0, len);
        }
        let (table, _) = self.table.as_mut().expect("just made");
        let bytes = bytemuck::cast_slice(&self.entries[first..end]);
        frame.write_buffer(table, (first * size) as u64, bytes)
    }

    /// The address of the table, which shaders read the materials through: 0 before any is added.
    fn address(&self) -> u64 {
        self.table.as_ref().map_or(0, |(table, _)| table.address())
    }
}

/// What culls the meshlets of the meshes drawn by them, in one table in the GPU's own memory,
/// which the culling reaches through one address, as `shaders/meshlets.slang` takes every address
/// from its push constants: each mesh's meshlets a run of entries, added as the mesh comes to be
/// drawable, and written in the frame's own commands, before its passes; the table made again,
/// twice the size, as it fills, the old one retiring until the GPU is done with it, from what's kept
/// of it on the CPU. A mesh removed or replaced leaves its run unused, which no mesh takes again:
/// the table grows by every mesh with meshlets a run adds, as a texture's slot isn't given out
/// again, until reusing runs is worth it, as for a game that streams levels in and out.
struct MeshletTable {
    /// The table, and how many entries it has room for.
    table: Option<(GpuBuffer, usize)>,
    entries: Vec<MeshletBounds>,
    /// How many of `entries` the table holds.
    written: usize,
}

impl MeshletTable {
    /// The fewest entries a table holds.
    const LEAST: usize = 1024;

    /// Adds `bounds`, a mesh's meshlets', and returns where they start.
    fn add(&mut self, bounds: &[MeshletBounds]) -> u32 {
        let first = self.entries.len();
        self.entries.extend_from_slice(bounds);
        u32::try_from(first).expect("fewer than 2^32 meshlets")
    }

    /// Writes the entries added since it last wrote into the table, which it makes, or makes again
    /// larger, as they need: one copy of those added, or of them all into a table made again.
    fn write(&mut self, device: &Arc<Device>, frame: &mut Frame<'_>) -> Result<()> {
        let len = self.entries.len();
        if self.written == len {
            return Ok(());
        }
        let size = size_of::<MeshletBounds>();
        if self.table.as_ref().is_none_or(|&(_, room)| room < len) {
            let room = len.next_power_of_two().max(Self::LEAST);
            let table = GpuBuffer::new(device, (room * size) as u64, "meshlet bounds")?;
            self.table = Some((table, room));
            self.written = 0;
        }
        let (table, _) = self.table.as_mut().expect("just made");
        let bytes = bytemuck::cast_slice(&self.entries[self.written..]);
        frame.write_buffer(table, (self.written * size) as u64, bytes)?;
        self.written = len;
        Ok(())
    }

    /// The address of the table: 0 before any meshlet is added.
    fn address(&self) -> u64 {
        self.table.as_ref().map_or(0, |(table, _)| table.address())
    }
}

/// Where a mesh's meshlets are in the [`MeshletTable`], and each level's.
pub(crate) struct PlacedMeshlets {
    /// The mesh's first entry.
    pub(crate) first: u32,
    /// Each level's meshlets among the mesh's: the first, and how many.
    pub(crate) levels: Vec<[u32; 2]>,
    /// The most meshlets a level has.
    pub(crate) most: u32,
}

/// An asset being streamed to the GPU.
struct Item {
    index: u32,
    name: String,
    /// The bytes it takes on the GPU, which every part copies some of.
    total: u64,
    /// Whether it has warned that it waits for room in the GPU's memory.
    warned: bool,
    asset: Asset,
}

enum Asset {
    Mesh {
        /// Its bytes, kept until the last part is copied.
        vertices: Vec<u8>,
        indices: Vec<u32>,
        /// How many bytes its vertices are, the first of `total`, and its indices the rest.
        vertex_bytes: u64,
        vertex_size: usize,
        lods: Lods,
        meshlets: Option<MeshletParts>,
        skinned: Option<u32>,
        /// Its buffers, once there's room for them: boxed, so that the queue's items stay small.
        buffers: Option<Box<(GpuBuffer, GpuBuffer)>>,
    },
    Texture {
        size: [u32; 2],
        texels: Texels,
        /// Its pixels, kept until the last part is copied, with the mip levels made for them once
        /// its image is.
        pixels: Vec<u8>,
        /// Whether its image is made, once there's room for it.
        made: bool,
    },
}

impl Item {
    /// Whether it's `kind`'s asset of handle `index`.
    fn is(&self, kind: Kind, index: u32) -> bool {
        let is_mesh = matches!(self.asset, Asset::Mesh { .. });
        self.index == index && is_mesh == (kind == Kind::Mesh)
    }

    /// Whether its buffers or image are made, so that it's being copied, or copied and waiting to
    /// be drawable.
    fn made(&self) -> bool {
        match &self.asset {
            Asset::Mesh { buffers, .. } => buffers.is_some(),
            Asset::Texture { made, .. } => *made,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mesh,
    Texture,
}

/// How many submissions of copies on the transfer queue may be unfinished as a frame begins
/// before the frame copies nothing ([`Frame::uploads_behind`]): with two frames in flight, a copy
/// engine that keeps up leaves at most the last frame's unfinished, so two means it has fallen a
/// frame or more behind, as over a slow link or under contention. Holding off then bounds what's
/// in flight to two frames' parts, their staging and their command buffers, with one more frame's
/// being recorded, whatever the frame rate.
const MAX_UPLOADS_BEHIND: u64 = 2;

/// The bytes a frame may copy of `budget` with `behind` submissions of copies unfinished as it
/// began: none once the transfer queue is [`MAX_UPLOADS_BEHIND`] behind, so that the frame neither
/// copies nor makes buffers or images, but takes what's drawable.
fn budget_behind(behind: u64, budget: u64) -> u64 {
    if behind >= MAX_UPLOADS_BEHIND {
        0
    } else {
        budget
    }
}

/// Whether an asset of `bytes` would take the GPU's memory past its budget, given `budget` and
/// how much is in `usage`.
fn exceeds(usage: u64, budget: u64, bytes: u64) -> bool {
    usage.saturating_add(bytes) > budget
}

/// Whether an asset of `bytes` waits for room in the GPU's memory, given `budget`, how much is in
/// `usage`, and whether anything of the queue is `copying`, made and not yet drawable: it waits
/// while it would take usage past the budget, as removals make room, unless nothing is copying,
/// when it goes ahead alone, so that a game comes up on a GPU whose memory is past the budget
/// already, as other processes' can take it, an asset at a time; the next asset past the budget
/// waits until this one is drawable.
fn waits(usage: u64, budget: u64, bytes: u64, copying: bool) -> bool {
    exceeds(usage, budget, bytes) && copying
}

/// Whether the asset `name`, of `bytes`, waits in the queue for room in the GPU's memory, as
/// [`waits`] decides from what `device` reports: warning the first time it's past the budget,
/// which `warned` keeps, whether it waits or goes ahead alone.
fn waits_for_room(
    device: &Device,
    name: &str,
    bytes: u64,
    copying: bool,
    warned: &mut bool,
) -> bool {
    let MemoryBudget { usage, budget } = device.memory_budget();
    if !exceeds(usage, budget, bytes) {
        return false;
    }
    let waits = waits(usage, budget, bytes, copying);
    if !*warned {
        let goes = if waits {
            "waits for room, until removals make some or nothing else is being copied"
        } else {
            "goes ahead alone, as nothing else is being copied"
        };
        warn!(
            code = "renderer.over_budget",
            asset = name,
            bytes,
            usage,
            budget,
            "{name:?}'s {bytes} bytes would take the {usage} in use in the GPU's memory past the \
             budget of {budget}: it {goes}"
        );
        *warned = true;
    }
    waits
}

/// Whether making a buffer or an image failed for want of memory, which [`room_for`] has the
/// asset wait out rather than end the renderer: the budget it's gated by is an estimate.
fn out_of_memory(error: &Error) -> bool {
    error.code() == "gpu.out_of_memory"
}

/// A shader's geometry, which its pipelines draw with: its module, named `name`, and the module of
/// its shadows if they're cut out, as [`shader::Added::shadow`] says; and its geometry's entry
/// point in each, or in `mesh`, a mesh shader's module apart from them, as [`GraphicsDesc::mesh`]
/// says.
struct Geometry<'a> {
    name: &'a str,
    spirv: &'a [u8],
    shadow: Option<&'a [u8]>,
    entry: &'a CStr,
    mesh: Option<&'a [u8]>,
}

impl Geometry<'_> {
    /// Its pipeline, into the game's view, with `fragment_main`, blending as `blend` says; and, if
    /// that writes depth, what draws its depth alone, by its geometry and, if its shadows are cut
    /// out, `shadow_main`.
    fn pipelines(
        &self,
        device: &Arc<Device>,
        blend: Blend,
    ) -> Result<(Pipeline, Option<DepthOnly>)> {
        let writes_depth = shader::writes_depth(blend);
        let pipeline = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: self.name,
                spirv: self.spirv,
                vertex: self.entry,
                mesh: self.mesh,
                fragment: Some(c"fragment_main"),
                format: Format::HDR.with_ids().with_motion().with_depth(),
                blend,
                depth_write: writes_depth,
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        // What's opaque casts shadows, drawn by its own geometry into the shadow maps, and draws
        // its depth so for the view's ambient occlusion, as the view draws it.
        let depth_only = |name: &str, shadow: bool| {
            Pipeline::graphics(
                device,
                &GraphicsDesc {
                    name,
                    spirv: self.shadow.unwrap_or(self.spirv),
                    vertex: self.entry,
                    mesh: self.mesh,
                    fragment: self.shadow.map(|_| c"shadow_main"),
                    format: Format::DEPTH,
                    blend: Blend::Opaque,
                    depth_write: true,
                    depth_bias: if shadow { shadow::DEPTH_BIAS } else { 0.0 },
                    depth_clamp: shadow,
                },
            )
        };
        let depth_only = writes_depth.then(|| {
            Ok(DepthOnly {
                shadow: depth_only(&format!("{} shadow", self.name), true)?,
                view: depth_only(&format!("{} depth", self.name), false)?,
            })
        });
        Ok((pipeline, depth_only.transpose()?))
    }
}

/// What draws the meshlets of the model shader `name` into the visibility buffer, from `meshlets`'
/// modules: through its mesh shader where the device draws through them, and through its vertex
/// shaders elsewhere.
fn visibility_pipeline(
    device: &Arc<Device>,
    name: &str,
    meshlets: &shader::Meshlets,
) -> Result<Pipeline> {
    let name = format!("{name} visibility");
    let (spirv, vertex, mesh, fragment) = if device.mesh_shaders() {
        let mesh = Some(meshlets.visibility_mesh.as_slice());
        let entries = (c"mesh_visibility_main", c"visibility_primitive_main");
        (
            meshlets.visibility_primitive.as_slice(),
            entries.0,
            mesh,
            entries.1,
        )
    } else {
        let entries = (c"meshlet_visibility_main", c"visibility_main");
        (meshlets.visibility.as_slice(), entries.0, None, entries.1)
    };
    Pipeline::graphics(
        device,
        &GraphicsDesc {
            name: &name,
            spirv,
            vertex,
            mesh,
            fragment: Some(fragment),
            format: Format::VISIBILITY.with_ids().with_depth(),
            blend: Blend::Opaque,
            depth_write: true,
            depth_bias: 0.0,
            depth_clamp: false,
        },
    )
}

/// What `made`, the buffers or image of the asset `name`, of `bytes`, gave: `None` if the GPU's
/// memory had no room, which warns the first time, `warned` keeping it, and leaves the asset to
/// wait, to be tried again as room comes; any other error as it is.
fn room_for<T>(made: Result<T>, name: &str, bytes: u64, warned: &mut bool) -> Result<Option<T>> {
    match made {
        Ok(made) => Ok(Some(made)),
        Err(error) if out_of_memory(&error) => {
            if !*warned {
                warn!(
                    code = "renderer.out_of_memory",
                    asset = name,
                    bytes,
                    "{name:?}'s {bytes} bytes don't fit the GPU's memory: {error:#}; it waits, \
                     and is tried again as room comes"
                );
                *warned = true;
            }
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// The buffers of the mesh `name`, `vertex_bytes` of vertices and `index_bytes` of indices.
fn mesh_buffers(
    device: &Arc<Device>,
    name: &str,
    vertex_bytes: u64,
    index_bytes: u64,
) -> Result<(GpuBuffer, GpuBuffer)> {
    let vertices = GpuBuffer::new(device, vertex_bytes, &format!("{name} vertices"))?;
    // Should the indices not fit, the vertices' buffer retires with the error.
    let indices = GpuBuffer::new(device, index_bytes, &format!("{name} indices"))?;
    Ok((vertices, indices))
}

impl MeshPass {
    /// A pass whose frames copy at most `budget` bytes each.
    pub(crate) fn new(device: &Arc<Device>, budget: u64) -> Result<Self> {
        Ok(Self {
            device: device.clone(),
            shaders: Vec::new(),
            meshes: Vec::new(),
            textures: Vec::new(),
            stand_ins: StandIns::new(device)?,
            streaming: VecDeque::new(),
            materials: MaterialTable {
                table: None,
                entries: Vec::new(),
            },
            budget,
            culling: cull::Culling::new(device)?,
            shading: ComputePipeline::new(
                device,
                "view shading",
                &shader::non_uniform(visibility::SPIRV),
                c"shade_main",
            )?,
            scene: scene::Kept::default(),
            meshlet_table: MeshletTable {
                table: None,
                entries: Vec::new(),
                written: 0,
            },
            skinned: Vec::new(),
            skinned_set: Vec::new(),
        })
    }

    /// The vertices of skinned mesh `index`, and how many there are, if it's drawable: the
    /// address of its buffer, which starts with its header, as its own draws read it.
    pub(crate) fn skinned_mesh(&self, index: u32) -> Option<(u64, u32)> {
        let mesh = self.meshes.get(index as usize)?.ready()?;
        Some((mesh.vertices.address(), mesh.skinned?))
    }

    /// Has the frame's draws of each of `moved`'s meshes, by its handle, read its vertices where
    /// the skin pass moved them, at the address beside it, which starts with its header; and of
    /// the rest, theirs.
    pub(crate) fn set_skinned(&mut self, moved: impl Iterator<Item = (u32, u64)>) {
        for index in self.skinned_set.drain(..) {
            self.skinned[index as usize] = 0;
        }
        for (index, address) in moved {
            let at = index as usize;
            if self.skinned.len() <= at {
                self.skinned.resize(at + 1, 0);
            }
            self.skinned[at] = address;
            self.skinned_set.push(index);
        }
    }

    /// Where mesh `index`, `mesh`, has its draws read its vertices this frame: where the skin pass
    /// moved them, if it did, or else its own buffer.
    fn vertices_address(&self, index: u32, mesh: &GpuMesh) -> u64 {
        match self.skinned.get(index as usize) {
            Some(&moved) if moved != 0 => moved,
            _ => mesh.vertices.address(),
        }
    }

    /// Makes the pipelines of the shaders in `uploads`, taking them, and drops those of the shaders
    /// it removes. A shader that has a pipeline already, compiled again, gets a new one in place of
    /// the old, which retires until the GPU is done with it; one whose new pipeline can't be made
    /// warns, and keeps the old; one removed stays removed. Then takes the changes to the scene,
    /// as frame `frame`'s, the next to begin, which writes them to the GPU.
    ///
    /// # Panics
    ///
    /// If they were taken from another `Shaders` than the pass's shaders before them.
    pub(crate) fn add_shaders(&mut self, uploads: &mut Uploads, frame: u64) -> Result<()> {
        let _zone = gg_core::zone!();
        for change in uploads.shaders.drain(..) {
            let added = match change {
                shader::Change::Added(added) => added,
                shader::Change::Removed(index) => {
                    let shader = self.shaders.get_mut(index as usize);
                    let shader =
                        shader.expect("a renderer draws with the shaders of one `Shaders`");
                    // The pipeline retires until the GPU is done with the frames that draw with it.
                    *shader = None;
                    continue;
                }
            };
            let index = added.index as usize;
            // A shader compiled again replaces one the pass has; any other is the next its
            // `Shaders` added.
            let follows = if added.replacement {
                index < self.shaders.len()
            } else {
                index == self.shaders.len()
            };
            assert!(
                follows,
                "a renderer draws with the shaders of one `Shaders`"
            );
            // Which the draws' sizes are checked against: its own types, since only the
            // `Shaders` that added it replaces it.
            if let Some(Some(old)) = self.shaders.get(index) {
                assert_eq!(
                    (old.vertex_size, old.params_size),
                    (added.vertex_size, added.params_size),
                    "shader {:?} is replaced by one for other types; a renderer draws with the \
                     shaders of one `Shaders`",
                    added.name
                );
            }
            let writes_depth = shader::writes_depth(added.blend);
            let device = &self.device;
            let geometry = Geometry {
                name: &added.name,
                spirv: &added.spirv,
                shadow: added.shadow.as_deref(),
                entry: c"vertex_main",
                mesh: None,
            };
            let made = geometry.pipelines(device, added.blend);
            // A model shader's meshlets, through the device's mesh shaders if it has them, with the
            // shader's own fragment shaders.
            let meshlets = (added.meshlets.as_deref()).map(|meshlets| {
                let name = format!("{} meshlets", added.name);
                let geometry = if device.mesh_shaders() {
                    Geometry {
                        name: &name,
                        spirv: &added.spirv,
                        shadow: added.shadow.as_deref(),
                        entry: c"mesh_main",
                        mesh: Some(&meshlets.mesh),
                    }
                } else {
                    Geometry {
                        name: &name,
                        spirv: &meshlets.vertices,
                        shadow: meshlets.vertices_shadow.as_deref(),
                        entry: c"meshlet_vertex_main",
                        mesh: None,
                    }
                };
                geometry.pipelines(device, added.blend)
            });
            // And into the visibility buffer.
            let visibility = (added.meshlets.as_deref())
                .map(|meshlets| visibility_pipeline(device, &added.name, meshlets));
            let made =
                made.and_then(|made| Ok((made, meshlets.transpose()?, visibility.transpose()?)));
            match self.shaders.get_mut(index) {
                Some(Some(old)) => match made {
                    Ok(((pipeline, depth_only), meshlets, visibility)) => {
                        (old.pipeline, old.depth_only) = (pipeline, depth_only);
                        (old.meshlets, old.visibility) = (meshlets, visibility);
                        old.writes_depth = writes_depth;
                    }
                    Err(error) => warn!(
                        code = error.code(),
                        shader = added.name,
                        "{error:#}; the shader keeps drawing as it did"
                    ),
                },
                Some(None) => unreachable!("its `Shaders` drops a removed shader's replacement"),
                None => {
                    let ((pipeline, depth_only), meshlets, visibility) = made?;
                    self.shaders.push(Some(GpuShader {
                        pipeline,
                        depth_only,
                        meshlets,
                        visibility,
                        // A name with a nul is labeled as far as it goes.
                        label: CString::new(added.name.split('\0').next().unwrap_or_default())
                            .expect("cut at its first nul"),
                        vertex_size: added.vertex_size,
                        params_size: added.params_size,
                        writes_depth,
                    }));
                }
            }
        }
        self.scene.apply(&mut uploads.scene, frame);
        Ok(())
    }

    /// Takes the changes to the meshes, textures and materials in `uploads`, writes the materials
    /// into their table and the scene's changes into its, and streams what's queued to the GPU,
    /// within the frame's budget, which must happen before the frame's passes. A texture
    /// added gets its slot, which shows a stand-in; its pixels, and a mesh's vertices and indices,
    /// queue up behind what was added before, and each frame copies parts of the queue in order,
    /// making an asset's buffers or image as its turn comes, if the GPU's memory has room for
    /// them, as [`waits`] and [`room_for`] decide, and otherwise leaving it to wait, warning once,
    /// while those after it go on; a frame the transfer queue has fallen behind copies nothing
    /// ([`budget_behind`]). A mesh draws once its buffers are there, and a texture's slot shows
    /// its image once that is; one replaced keeps drawing what it had until then, and whatever of
    /// it was still being streamed is dropped. A mesh or texture removed draws nothing, or shows
    /// its stand-in, from this frame on, and whatever of it was queued is dropped.
    ///
    /// # Panics
    ///
    /// If they were taken from another `Meshes` or `Textures` than the pass's meshes and textures
    /// before them.
    pub(crate) fn upload(&mut self, frame: &mut Frame<'_>, uploads: &mut Uploads) -> Result<()> {
        let _zone = gg_core::zone!();
        self.stand_ins.write(frame)?;
        for change in uploads.meshes.drain(..) {
            self.mesh_changed(change);
        }
        for change in uploads.textures.drain(..) {
            self.texture_changed(frame, change)?;
        }
        (self.materials).write(&self.device, frame, &mut uploads.materials)?;
        self.scene.write(&self.device, frame)?;
        self.stream(frame)?;
        // The meshlets of the meshes that came to be drawable.
        self.meshlet_table.write(&self.device, frame)
    }

    /// The address of the table of materials, which the frame's data carries: 0 before any is
    /// added.
    pub(crate) fn materials_address(&self) -> u64 {
        self.materials.address()
    }

    /// Takes a change to the meshes: queues what was added, or removes a mesh.
    fn mesh_changed(&mut self, change: Change) {
        let added = match change {
            Change::Added(added) => added,
            Change::Removed(index) => {
                let held = self.meshes.get_mut(index as usize);
                let held = held.expect("a renderer draws the meshes of one `Meshes`");
                // The buffers retire until the GPU is done with the frames that draw them.
                *held = Held::Removed;
                self.streaming.retain(|item| !item.is(Kind::Mesh, index));
                return;
            }
        };
        let index = added.index as usize;
        // A mesh replaced is one the pass has; any other is the next its `Meshes` added.
        let follows = if added.replacement {
            index < self.meshes.len()
        } else {
            index == self.meshes.len()
        };
        assert!(follows, "a renderer draws the meshes of one `Meshes`");
        if !added.replacement {
            self.meshes.push(Held::Waiting);
        }
        debug_assert!(
            !matches!(self.meshes[index], Held::Removed),
            "its `Meshes` drops a removed mesh's replacement"
        );
        // In place of any of the mesh still queued, whose buffers go with it.
        self.streaming
            .retain(|item| !item.is(Kind::Mesh, added.index));
        let vertex_bytes = added.vertices.len() as u64;
        let total = vertex_bytes + (added.indices.len() * size_of::<u32>()) as u64;
        self.streaming.push_back(Item {
            index: added.index,
            name: added.name,
            total,
            warned: false,
            asset: Asset::Mesh {
                vertices: added.vertices,
                indices: added.indices,
                vertex_bytes,
                vertex_size: added.vertex_size,
                lods: added.lods,
                meshlets: added.meshlets,
                skinned: added.skinned,
                buffers: None,
            },
        });
    }

    /// Takes a change to the textures: makes a slot's texture, queues an image, or removes a
    /// texture.
    fn texture_changed(&mut self, frame: &mut Frame<'_>, change: texture::Change) -> Result<()> {
        let Self {
            device,
            textures,
            stand_ins,
            streaming,
            ..
        } = self;
        match change {
            texture::Change::Reserved { index, name, kind } => {
                assert_eq!(
                    index as usize,
                    textures.len(),
                    "a renderer samples the textures of one `Textures`"
                );
                // In the slot of its handle's index, which shaders sample it by.
                let texture = GpuTexture::placed(device, index, stand_ins.of(kind), &name)?;
                textures.push(TextureSlot {
                    texture,
                    kind,
                    removed: false,
                    shown: 0,
                });
            }
            texture::Change::Image(added) => {
                let slot = textures.get_mut(added.index as usize);
                let slot = slot.expect("a renderer samples the textures of one `Textures`");
                debug_assert!(
                    !slot.removed,
                    "its `Textures` drops a removed texture's images"
                );
                // In place of any image of the texture still queued, whose image, if it was made,
                // retires with whatever was copied into it.
                streaming.retain(|item| !item.is(Kind::Texture, added.index));
                slot.texture.discard();
                let (format, mipmapped) = (added.texels.format(), added.texels.mipmapped());
                streaming.push_back(Item {
                    index: added.index,
                    name: added.name,
                    total: format.bytes(added.size, mipmapped),
                    warned: false,
                    asset: Asset::Texture {
                        size: added.size,
                        texels: added.texels,
                        pixels: added.pixels,
                        made: false,
                    },
                });
            }
            texture::Change::Removed(index) => {
                let slot = textures.get_mut(index as usize);
                let slot = slot.expect("a renderer samples the textures of one `Textures`");
                debug_assert!(!slot.removed, "its `Textures` removes a texture once");
                streaming.retain(|item| !item.is(Kind::Texture, index));
                frame.hide_texture(&mut slot.texture, stand_ins.of(slot.kind));
                slot.removed = true;
                slot.shown += 1;
            }
        }
        Ok(())
    }

    /// Copies the next parts of what's queued, in order, within the frame's budget, or none if the
    /// transfer queue has fallen behind, and takes what has come to be drawable off the queue,
    /// into its place.
    fn stream(&mut self, frame: &mut Frame<'_>) -> Result<()> {
        if self.streaming.is_empty() {
            return Ok(());
        }
        let _zone = gg_core::zone!();
        let budget = budget_behind(frame.uploads_behind(), self.budget);
        let mut copying = self.streaming.iter().any(Item::made);
        let mut spent = 0;
        let mut i = 0;
        while i < self.streaming.len() {
            if self.advance(frame, i, budget, &mut spent, &mut copying)? {
                self.streaming.remove(i);
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// Streams the `i`th queued asset on: makes its buffers or image if there's room, which sets
    /// `copying`, copies its next parts while the frame's bytes `spent` are under `budget`, and
    /// once every part is there, takes it for drawing. Returns whether it's drawable, and so done.
    fn advance(
        &mut self,
        frame: &mut Frame<'_>,
        i: usize,
        budget: u64,
        spent: &mut u64,
        copying: &mut bool,
    ) -> Result<bool> {
        let Self {
            device,
            meshes,
            textures,
            streaming,
            meshlet_table,
            ..
        } = self;
        let item = &mut streaming[i];
        match &mut item.asset {
            Asset::Mesh {
                vertices,
                indices,
                vertex_bytes,
                vertex_size,
                lods,
                meshlets,
                skinned,
                buffers,
            } => {
                if buffers.is_none() {
                    if *spent >= budget
                        || waits_for_room(
                            device,
                            &item.name,
                            item.total,
                            *copying,
                            &mut item.warned,
                        )
                    {
                        return Ok(false);
                    }
                    let index_bytes = item.total - *vertex_bytes;
                    let made = mesh_buffers(device, &item.name, *vertex_bytes, index_bytes);
                    let Some(made) = room_for(made, &item.name, item.total, &mut item.warned)?
                    else {
                        return Ok(false);
                    };
                    *buffers = Some(Box::new(made));
                    *copying = true;
                }
                let (vertex_buffer, index_buffer) = &mut **buffers.as_mut().expect("just made");
                let copied = |vertices: &GpuBuffer, indices: &GpuBuffer| {
                    vertices.streamed() + indices.streamed()
                };
                while *spent < budget && copied(vertex_buffer, index_buffer) < item.total {
                    let part = if vertex_buffer.streamed() < *vertex_bytes {
                        frame.stream_buffer(vertex_buffer, vertices, budget - *spent)?
                    } else {
                        let bytes: &[u8] = bytemuck::cast_slice(indices);
                        frame.stream_buffer(index_buffer, bytes, budget - *spent)?
                    };
                    *spent += part;
                }
                if copied(vertex_buffer, index_buffer) < item.total {
                    return Ok(false);
                }
                // The last part is copied: the bytes go, whatever else waits.
                *vertices = Vec::new();
                *indices = Vec::new();
                let vertices_ready = frame.finish_buffer(vertex_buffer)?;
                let indices_ready = frame.finish_buffer(index_buffer)?;
                if !(vertices_ready && indices_ready) {
                    return Ok(false);
                }
                let (vertices, indices) = *buffers.take().expect("just there");
                let mesh = GpuMesh {
                    vertices,
                    indices,
                    lods: std::mem::replace(lods, Lods::whole(0)),
                    vertex_size: *vertex_size,
                    meshlets: meshlets.take().map(|parts| PlacedMeshlets {
                        first: meshlet_table.add(&parts.bounds),
                        most: (parts.levels.iter())
                            .map(|&[_, count]| count)
                            .max()
                            .unwrap_or(0),
                        levels: parts.levels,
                    }),
                    skinned: *skinned,
                };
                let held = &mut meshes[item.index as usize];
                // Which the draws' sizes are checked against: its own type, since only the
                // `Meshes` that added it replaces it.
                if let Some(old) = held.ready() {
                    assert_eq!(
                        old.vertex_size, mesh.vertex_size,
                        "mesh {:?} is replaced by vertices of another type; a renderer draws the \
                         meshes of one `Meshes`",
                        item.name
                    );
                }
                // The old buffers, if any, retire until the GPU is done with the frames that
                // draw them.
                *held = Held::Ready(mesh);
                Ok(true)
            }
            Asset::Texture {
                size,
                texels,
                pixels,
                made,
            } => {
                let slot = &mut textures[item.index as usize];
                let texture = &mut slot.texture;
                if !*made {
                    if *spent >= budget
                        || waits_for_room(
                            device,
                            &item.name,
                            item.total,
                            *copying,
                            &mut item.warned,
                        )
                    {
                        return Ok(false);
                    }
                    let [width, height] = *size;
                    // A replacement may be compressed where the image it replaces wasn't, or the
                    // other way round, as its `Textures` keeps only its kind.
                    let refilled =
                        texture.refill(width, height, texels.format(), texels.mipmapped());
                    if room_for(refilled, &item.name, item.total, &mut item.warned)?.is_none() {
                        return Ok(false);
                    }
                    // Once the image is there, so that a refill tried again doesn't make them again.
                    texture::add_levels(pixels, *size, *texels);
                    *made = true;
                    *copying = true;
                }
                while *spent < budget && texture.streamed() < item.total {
                    let part = frame.stream_texture(texture, pixels, budget - *spent)?;
                    *spent += part;
                }
                if texture.streamed() < item.total {
                    return Ok(false);
                }
                // The last part is copied: the pixels go, whatever else waits.
                *pixels = Vec::new();
                let finished = frame.finish_texture(texture)?;
                slot.shown += u64::from(finished);
                Ok(finished)
            }
        }
    }

    /// What `texture`'s slot shows, if it's an image of its own rather than a stand-in: its size,
    /// and how many times the slot has come to show another image, which tells this one apart
    /// from those it showed before. `None` for a texture that isn't the pass's.
    pub(crate) fn texture_shown(&self, texture: crate::Texture) -> Option<([u32; 2], u64)> {
        let slot = self.textures.get(texture.index as usize)?;
        let size = slot.texture.size();
        (size != [0, 0]).then_some((size, slot.shown))
    }

    /// How many bytes of the `item`th queued asset are copied.
    fn copied(&self, item: &Item) -> u64 {
        match &item.asset {
            Asset::Mesh { buffers, .. } => buffers
                .as_ref()
                .map_or(0, |buffers| buffers.0.streamed() + buffers.1.streamed()),
            Asset::Texture { .. } => self.textures[item.index as usize].texture.streamed(),
        }
    }

    /// What's being streamed.
    pub(crate) fn streaming(&self) -> Streaming {
        Streaming {
            assets: self.streaming.len(),
            bytes: (self.streaming.iter())
                .map(|item| item.total - self.copied(item))
                .sum(),
        }
    }

    /// Copies `draws`' transforms, IDs and parameters into the frame, each array once, for every
    /// pass that draws them, and for culling them: where they are.
    pub(crate) fn upload_draws(frame: &mut Frame<'_>, draws: &Draws) -> Result<Uploaded> {
        // Each array in an upload of its own, so that each starts 16-byte aligned.
        let mut address = |bytes: &[u8]| -> Result<u64> {
            Ok(if bytes.is_empty() {
                0
            } else {
                frame.upload(bytes)?.address()
            })
        };
        Ok(Uploaded {
            transforms: address(bytemuck::cast_slice(&draws.transforms))?,
            ids: address(bytemuck::cast_slice(&draws.ids))?,
            params: address(&draws.params)?,
        })
    }

    /// Draws `runs` of `draws` and of the scene over the whole of the pass's target, `view` pixels
    /// across and down, where `view_projection` takes the world to clip space, from their instances
    /// as `uploaded` has the draws', for shaders that read the frame's data, a [`FrameData`], at
    /// the address `frame`: the culled runs first, then the rest, each in the order the draws say,
    /// the scene's runs where they place them. A run that's culled draws what the culling's view
    /// kept of it, a draw for each of its mesh's levels; any other is one instanced draw, or, of a
    /// mesh with levels of detail, one for each stretch of its instances in a row that draw at one
    /// level, in order. The scene's runs draw only as they're culled. A run of a mesh that isn't
    /// drawable yet, or was removed, or with a shader that was, draws nothing. Each draws as `way`
    /// says; and an emitter's particles, in their place among the rest, as `particles` draws them,
    /// where the pass shades, and nothing without them. Returns what it drew of the runs it didn't
    /// cull, which the culling counts of the rest.
    ///
    /// # Panics
    ///
    /// If a mesh drawn isn't one of the pass's `Meshes`, a shader isn't one of its `Shaders`, a
    /// mesh's vertices or a draw's parameters aren't the size its shader reads, a run has more
    /// instances than Vulkan's 32-bit count, or the pass isn't into what `way` draws into: an
    /// image of [`Format::HDR`], or a visibility buffer, with IDs and a depth image, or depth
    /// alone.
    #[expect(
        clippy::too_many_arguments,
        reason = "what a pass draws by, each apart"
    )]
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        view_projection: Mat4,
        frame: u64,
        view: [u32; 2],
        draws: &Draws,
        uploaded: Uploaded,
        runs: Runs<'_>,
        way: Way,
        particles: Option<&ParticleDraws<'_>>,
    ) -> Result<DrawCounts> {
        let _zone = gg_core::zone!();
        let mut drawn = DrawCounts::default();
        let lod_view = LodView::new(&view_projection, view);
        // What's set, which a pipeline's binding leaves as it is.
        let (mut bound, mut faces) = (None, None);
        // The draws before the scene's place, the scene's runs, then the draws after.
        let scene_at = draws.scene_at.unwrap_or(0);
        let order = || {
            let frame_runs = |runs: std::ops::Range<usize>| runs.map(Drawn::Frame);
            (frame_runs(0..scene_at))
                .chain((0..self.scene.runs().len()).map(Drawn::Scene))
                .chain(frame_runs(scene_at..draws.runs.len()))
        };
        let (culled, culled_drawn, rest) = match runs {
            Runs::Culled(plan, slot) => (Some((plan, slot)), true, false),
            Runs::CulledThenRest(plan, slot) => (Some((plan, slot)), true, true),
            // The culled runs aren't drawn, so their slot is never read.
            Runs::Rest(plan) => (Some((plan, FIRST)), false, true),
            Runs::All => (None, false, true),
        };
        // The culled runs, then the rest, which don't hide them.
        let culled_order = (culled.filter(|_| culled_drawn))
            .into_iter()
            .flat_map(|_| order().map(|run| (run, true)));
        let rest_order = rest.then(order).into_iter().flatten();
        for (drawn_run, culling) in culled_order.chain(rest_order.map(|run| (run, false))) {
            // Its mesh, its shader, the faces it draws, the draws' run, if it's theirs, and its
            // index among the runs culled, if it's culled.
            let (run, culled_run) = match drawn_run {
                Drawn::Frame(i) => {
                    let run = &draws.runs[i];
                    let culled_run = culled.and_then(|_| self.culling.of_run(i));
                    ((run.mesh, run.shader, run.faces, Some(run)), culled_run)
                }
                Drawn::Scene(i) => {
                    let run = &self.scene.runs()[i];
                    let culled_run = culled.and_then(|_| self.culling.of_scene(i));
                    ((run.mesh, run.shader, run.faces, None), culled_run)
                }
            };
            let (mesh, shader, run_faces, run) = run;
            // Each in its turn, and the scene's runs only as they're culled.
            if culled_run.is_some() != culling || (run.is_none() && culled_run.is_none()) {
                continue;
            }
            let mesh_index = mesh;
            let mesh = self
                .meshes
                .get(mesh as usize)
                .expect("a mesh of the renderer's `Meshes`");
            let shader_index = shader;
            let shader = self
                .shaders
                .get(shader as usize)
                .expect("a shader of the renderer's `Shaders`");
            let (Some(mesh), Some(shader)) = (mesh.ready(), shader) else {
                continue;
            };
            // An emitter's particles, which are never culled, on the quad, whose indices they take.
            if let Some(index) = run.and_then(|run| run.particles) {
                if let (Way::Shaded { .. }, Some(particles)) = (way, particles) {
                    pass.begin_label(c"particles");
                    drawn += particles.draw(pass, &mesh.indices, index, view_projection, frame);
                    pass.end_label();
                    // Its pipeline is bound and its faces are set in place of the runs'.
                    (bound, faces) = (None, None);
                }
                continue;
            }
            // A run the culling draws by its meshlets, by its shader's meshlet geometry.
            let by_meshlets = culled_run.is_some_and(|run| self.culling.by_meshlets(run));
            let (geometry, depth_only) = match &shader.meshlets {
                Some((geometry, depth_only)) if by_meshlets => (geometry, depth_only),
                _ => (&shader.pipeline, &shader.depth_only),
            };
            // Such a run of a model shader's draws into the visibility buffer where the view has
            // one.
            let visible = shader.visibility.as_ref().filter(|_| by_meshlets);
            let pipeline = match (way, visible) {
                (Way::Shaded { beside_visibility }, visible) => {
                    if beside_visibility && visible.is_some() {
                        continue;
                    }
                    geometry
                }
                (Way::Shadows, _) => {
                    let Some(depth_only) = depth_only else {
                        continue;
                    };
                    &depth_only.shadow
                }
                (Way::Depth { beside_visibility }, visible) => {
                    let Some(depth_only) = depth_only else {
                        continue;
                    };
                    if beside_visibility && visible.is_some() {
                        continue;
                    }
                    &depth_only.view
                }
                (Way::Visibility, Some(visible)) => visible,
                (Way::Visibility, None) => continue,
            };
            // A draw pairs a mesh and parameters with a shader for their types, but handles from
            // another `Meshes` or `Shaders` could pair them with one that reads past them.
            let params =
                run.map(|run| (run.params.len(), shader.params_size * run.instances.len()));
            let (has, takes) = params.unwrap_or_default();
            assert_eq!(
                (mesh.vertex_size, has),
                (shader.vertex_size, takes),
                "a mesh draws with a shader for its vertex type, and the parameters that shader \
                 takes, from the renderer's `Meshes` and `Shaders`"
            );
            if bound != Some((shader_index, by_meshlets)) {
                pass.set_pipeline(pipeline);
                bound = Some((shader_index, by_meshlets));
            }
            if faces != Some(run_faces) {
                pass.set_faces(run_faces);
                faces = Some(run_faces);
            }
            let push = Push {
                clip_from_world: view_projection,
                vertices: self.vertices_address(mesh_index, mesh),
                // Where each draw's instances are, which it sets.
                transforms: 0,
                ids: 0,
                params: 0,
                view: view.map(|side| side as f32),
                frame,
                // A draw of meshlets sets them, and a culled draw of the game's view its instances'
                // transforms the frame before.
                meshlets: 0,
                draw: 0,
                previous: 0,
            };
            pass.begin_label(&shader.label);
            match (culled, culled_run, run) {
                (Some(culled), Some(culled_run), _) => {
                    let visibility = way == Way::Visibility;
                    self.draw_culled(pass, mesh, push, culled, culled_run, visibility);
                }
                (_, _, Some(run)) => {
                    let instances = Instances {
                        run,
                        uploaded,
                        params_size: shader.params_size,
                    };
                    drawn += draw_run(pass, mesh, push, instances, &lod_view, &draws.transforms);
                }
                _ => unreachable!("the scene's runs draw only as they're culled"),
            }
            pass.end_label();
        }
        Ok(drawn)
    }
}

/// How a pass draws the runs it draws, as [`MeshPass::draw`] draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Way {
    /// Shaded as they draw, by their shaders' pipelines, into colors of high dynamic range: every
    /// run, or, beside a visibility buffer, all but those drawn into it, which it shades.
    Shaded { beside_visibility: bool },
    /// Their shadows, into a shadow map, by the shadow pipelines of those whose shaders write depth,
    /// and nothing of the rest.
    Shadows,
    /// Their depth alone, as the view draws it, into the depth the view's ambient occlusion takes,
    /// by the depth pipelines of those whose shaders write depth: every such run, or, beside a
    /// visibility buffer, all but those drawn into it, whose depth the view's holds; and nothing
    /// of the rest.
    Depth { beside_visibility: bool },
    /// Into a visibility buffer: the culled runs drawn by their meshlets whose shaders have a
    /// pipeline for it, as the opaque and masked model shaders do, and nothing of the rest.
    Visibility,
}

/// Which of a frame's runs a pass draws, as [`MeshPass::draw`] draws them.
#[derive(Clone, Copy)]
pub(crate) enum Runs<'p> {
    /// What the culling's view in the slot given kept of the runs it culled, alone: the game's
    /// view's first pass, or a shadow cascade's.
    Culled(&'p CullPlan, usize),
    /// What the culling's view in the slot given kept of the runs it culled, then the rest: the
    /// game's view's second pass.
    CulledThenRest(&'p CullPlan, usize),
    /// The runs the culling doesn't cull alone, what blends: the game's view's pass after its fog,
    /// which fogs what the passes before it drew.
    Rest(&'p CullPlan),
    /// Every run, on the CPU's levels: the canvas's, or a view's whose frame culls none.
    All,
}

/// A run a pass draws: the draws' run of that index, or the scene's.
#[derive(Clone, Copy)]
enum Drawn {
    Frame(usize),
    Scene(usize),
}

/// How many pixels a level's error may span on screen for a draw to use the level: the least that
/// shows, as the pixel is the least a view resolves.
const LOD_ERROR_PIXELS: f32 = 1.0;

/// What choosing a mesh's level of detail needs of the view it's drawn into, found once for all of
/// a pass's draws from the matrix, `C`, that takes the world to clip space.
///
/// `C` is a projection times a view matrix that is rigid, so that it keeps lengths. In both of the
/// engine's projections a point's clip W is its distance ahead of the camera, which `C`'s fourth
/// row dotted with the point gives: perspective's own fourth row is (0, 0, -1, 0), W the view
/// space's -Z, and orthographic's is (0, 0, 0, 1), W always 1. The rows that make clip X and Y are
/// the projection's scale on that axis times a row of the view's rotation, so the length of such a
/// row's XYZ is the scale alone, however the camera turns: the clip units, half the view across,
/// that a world unit spans where W is 1, and half the view's size in pixels times that is pixels.
struct LodView {
    /// `C`'s fourth row: a point's clip W is its dot product with the point, with a W of 1.
    w: Vec4,
    /// The length of `w`'s XYZ: how much W changes with a world unit's distance, 1 under
    /// perspective and 0 under orthographic.
    w_slope: f32,
    /// The pixels a world unit spans where W is 1, the larger of across and down.
    pixels_per_unit: f32,
}

impl LodView {
    fn new(view_projection: &Mat4, view: [u32; 2]) -> Self {
        let w = view_projection.row(3);
        let pixels = |axis: usize| {
            let clip_units = view_projection.row(axis).truncate().length();
            clip_units * view[axis] as f32 / 2.0
        };
        Self {
            w,
            w_slope: w.truncate().length(),
            pixels_per_unit: pixels(0).max(pixels(1)),
        }
    }

    /// The coarsest of `lods`' levels whose error, as `transform` places the mesh in the world,
    /// spans at most [`LOD_ERROR_PIXELS`] on screen where the mesh is nearest, or level 0 if none
    /// does, or the mesh reaches the plane of the camera or crosses it.
    ///
    /// `transform` scales the mesh's units by the longest of its axes, which is how it scales the
    /// error and the sphere. The error spans `error * scale * pixels_per_unit / near` pixels, where
    /// `near` is the W of the sphere's nearest point, W less the radius, at most.
    fn level(&self, lods: &Lods, transform: &Mat4) -> usize {
        let axes = [transform.x_axis, transform.y_axis, transform.z_axis];
        let scale_squared = axes.map(|axis| axis.truncate().length_squared());
        let scale = scale_squared[0]
            .max(scale_squared[1])
            .max(scale_squared[2])
            .sqrt();
        let center = transform.transform_point3(lods.center);
        let w = self.w.truncate().dot(center) + self.w.w;
        let near = w - lods.radius * scale * self.w_slope;
        if near <= 0.0 {
            return 0;
        }
        // The levels' errors never decrease, so the last to pass is the coarsest. A NaN passes
        // none.
        let allowed = LOD_ERROR_PIXELS * near / (scale * self.pixels_per_unit);
        (lods.levels.iter())
            .rposition(|lod| lod.error <= allowed)
            .unwrap_or(0)
    }
}

/// A run's instances, where they were uploaded.
#[derive(Clone, Copy)]
struct Instances<'a> {
    run: &'a Run,
    uploaded: Uploaded,
    /// The size of one instance's parameters.
    params_size: usize,
}

/// Draws `instances` of `mesh` directly, with what `push` has but for the instances, which each
/// draw sets: one instanced draw, or, of a mesh with levels of detail, one for each stretch of
/// instances in a row that draw at the level `view` picks for them, by their `transforms`, in their
/// order, as what blends and the canvas draw in order. Returns what it drew.
fn draw_run(
    pass: &mut Pass<'_, '_>,
    mesh: &GpuMesh,
    push: Push,
    instances: Instances<'_>,
    view: &LodView,
    transforms: &[Mat4],
) -> DrawCounts {
    let Instances {
        run,
        uploaded,
        params_size,
    } = instances;
    let lods = &mesh.lods;
    let level_at = |i: usize| match lods.levels.as_slice() {
        [_] => 0,
        _ => view.level(lods, &transforms[i]),
    };
    let mut drawn = DrawCounts::default();
    let (mut start, end) = (run.instances.start, run.instances.end);
    let mut level = level_at(start);
    for i in start + 1..=end {
        let next = if i < end { level_at(i) } else { usize::MAX };
        if next == level {
            continue;
        }
        let count =
            u32::try_from(i - start).expect("a run's instances fit the 32-bit count Vulkan takes");
        // Its parameters are its run's from where its instances are in the run.
        let params = if params_size == 0 {
            0
        } else {
            let at = run.params.start + (start - run.instances.start) * params_size;
            uploaded.params + at as u64
        };
        let push = Push {
            transforms: uploaded.transforms + (start * size_of::<Mat4>()) as u64,
            ids: uploaded.ids + (start * size_of::<[u32; 2]>()) as u64,
            params,
            ..push
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        let lod = lods.levels[level];
        pass.draw_indexed(&mesh.indices, lod.start, lod.count, 0, count);
        drawn += DrawCounts {
            draws: 1,
            instances: count as usize,
            triangles: lod.count as usize / 3 * count as usize,
        };
        (start, level) = (i, next);
    }
    drawn
}

/// What the mesh pass pushes before each draw, which a mesh shader's push constants read the
/// start of, as [`Shaders::add`](crate::Shaders::add) describes, and which the engine's mesh
/// shaders declare as `Push` in `shaders/mesh.slang`. The matrix goes first, so that every offset
/// is the same under every layout Slang gives push constants; the IDs before the parameters, so
/// that a shader that takes none still ends its struct after the last member it reads; and the
/// view's size, the frame's data and a draw of meshlets after them, each added at the end, so that
/// it moved none of the offsets games' shaders declare before it, though a shader that takes no
/// parameters can't read them.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Push {
    /// From the world to clip space, column-major.
    pub(crate) clip_from_world: Mat4,
    /// The address of the mesh's vertices.
    pub(crate) vertices: u64,
    /// The address of the instances' transforms.
    pub(crate) transforms: u64,
    /// The address of the instances' IDs, each the low and high halves of a 64-bit number.
    pub(crate) ids: u64,
    /// The address of the instances' parameters, or 0 for a shader that takes none.
    pub(crate) params: u64,
    /// The size of the view the pass draws into, in pixels, across then down.
    pub(crate) view: [f32; 2],
    /// The address of the frame's data, a [`FrameData`].
    pub(crate) frame: u64,
    /// For a draw of the meshlets a view kept of a mesh, through the model shaders' meshlet
    /// geometry, the address of the draw as the culling wrote it, which those it kept follow; 0
    /// for any other draw.
    pub(crate) meshlets: u64,
    /// For a draw of them into the visibility buffer, which run of the frame's culled runs it is
    /// and in which of the game's view's passes, 1 past twice the run's index, and that pass's 1
    /// past it, as the visibility buffer holds it, so that 0 is none: 0 for any other draw.
    pub(crate) draw: u32,
    /// How many transforms past each instance's, in the array `transforms` points into, its
    /// transform the frame before is, which the game's view moves what it draws by: for the view's
    /// culled draws, where the culling kept those, and 0 for the rest, which the renderer knows no
    /// transform the frame before of, as a layer's draws, made each frame, so that they read their
    /// own and move with the camera alone.
    pub(crate) previous: u32,
}

/// What every draw's shader may read of the frame, through [`Push::frame`]: `Frame` in
/// `shaders/mesh.slang`, field for field. Fields are added at its end, so that a shader that reads
/// those before them keeps to the contract.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
pub(crate) struct FrameData {
    /// Where the view sees from, in the world.
    pub(crate) camera: Vec3,
    /// The time the view shows, in seconds, as [`RenderData::time`](crate::RenderData::time)
    /// gives it.
    pub(crate) time: f32,
    /// What lit shading multiplies light by, as
    /// [`RenderData::exposure`](crate::RenderData::exposure) gives it.
    pub(crate) exposure: f32,
    /// How many lights the frame has, as [`RenderData::lights`](crate::RenderData::lights) gives
    /// them.
    pub(crate) light_count: u32,
    /// The address of its lights, a [`GpuLight`](crate::light::GpuLight) each, in the order
    /// [`Lighting::lights`](crate::cluster::Lighting::lights) has them, or 0 for none: for the
    /// game's view, if a light is binned, their copies, which the clusters follow, as
    /// [`Lighting::bin`](crate::cluster::Lighting::bin) returns them.
    pub(crate) lights: u64,
    /// The address of the table of materials, each by its index, or 0 before any is added.
    pub(crate) materials: u64,
    /// Where lit shading sees from: the camera's position and 1 for a perspective, or the way back
    /// toward it and 0 for an orthographic projection, which sees along one way, so that the way
    /// to the eye from a point `p` is `eye.xyz - p * eye.w`.
    pub(crate) eye: Vec4,
    /// The frame's sky and its prefiltered light, as [`SkyData`](crate::sky::SkyData) has them.
    pub(crate) sky: crate::sky::SkyData,
    /// The shadows of the light that casts them, if one does, as
    /// [`ShadowData`](crate::shadow::ShadowData) has them.
    pub(crate) shadows: crate::shadow::ShadowData,
    /// The clusters the lights are binned into, as [`ClusterData`](crate::cluster::ClusterData)
    /// has them.
    pub(crate) clusters: crate::cluster::ClusterData,
    /// The slot of the game's view's ambient occlusion in the bindless set, an image of the view's
    /// size, the fraction of the light from all around that reaches each pixel, as
    /// [`AmbientOcclusion`](crate::AmbientOcclusion) says; 0 for none, which shaders take as 1,
    /// as for every pass but the view's shading.
    pub(crate) ambient_occlusion: u32,
    pub(crate) unused: u32,
    /// The height fog the view is in, as [`FogData`](crate::fog::FogData) has it.
    pub(crate) fog: crate::fog::FogData,
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use gg_core::math::Quat;

    use super::*;
    use crate::{Mipmaps, Projection, Viewpoint, canvas_projection};

    const TRIANGLE: [[f32; 3]; 3] = [[0.0, -0.5, 0.0], [0.5, 0.5, 0.0], [-0.5, 0.5, 0.0]];

    /// Three triangles of `TRIANGLE`'s corners, which levels of detail pick among.
    const THREE: [u32; 9] = [0, 1, 2, 2, 1, 0, 1, 2, 0];

    /// Levels of detail that each start where they please and have `errors`, round a sphere of
    /// `radius` around the origin.
    fn lods(errors: &[f32], radius: f32) -> Lods {
        let level = |&error| Lod {
            start: 0,
            count: 3,
            error,
        };
        Lods {
            levels: errors.iter().map(level).collect(),
            center: Vec3::ZERO,
            radius,
        }
    }

    /// The meshes added or replaced, waiting to be taken, in order.
    fn adds(meshes: &Meshes) -> Vec<&Added> {
        (meshes.changes.iter())
            .filter_map(|change| match change {
                Change::Added(added) => Some(added),
                Change::Removed(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_mesh_is_whole_triangles_of_its_own_vertices() {
        let mut meshes = Meshes::default();
        let bad = [&[][..], &[0, 1], &[0, 1, 2, 0], &[0, 1, 3]];
        for indices in bad {
            let error = meshes
                .add("broken", &TRIANGLE, indices)
                .expect_err("a broken mesh");
            assert_eq!(error.code(), "renderer.bad_mesh", "{indices:?}");
            assert!(error.to_string().contains("\"broken\""), "{error}");
        }
        let mut add = |indices: &[u32]| meshes.add("a", &TRIANGLE, indices).map(|mesh| mesh.index);
        // After the engine's quad.
        assert_eq!(add(&[0, 1, 2]).ok(), Some(1));
        assert_eq!(add(&[2, 1, 0]).ok(), Some(2));
    }

    #[test]
    fn a_mesh_replaced_keeps_its_handle_and_the_place_of_triangles_waiting() {
        let mut meshes = Meshes::default();
        let a = meshes.add("a", &TRIANGLE, &[0, 1, 2]).expect("a");
        let waiting = |meshes: &Meshes| -> Vec<(u32, bool, Vec<u32>)> {
            (adds(meshes).into_iter())
                .map(|added| (added.index, added.replacement, added.indices.clone()))
                .collect()
        };
        // Still waiting as it was added, which takes the triangles instead.
        let square = [TRIANGLE.as_slice(), &[[1.0; 3]]].concat();
        meshes
            .replace("a", a, &square, &[0, 1, 2, 2, 1, 3])
            .expect("others");
        let quad = (0, false, sprite::INDICES.to_vec());
        assert_eq!(waiting(&meshes), [quad, (1, false, vec![0, 1, 2, 2, 1, 3])]);
        // Once a frame has taken it, in place of the one the renderer has.
        meshes.changes.clear();
        meshes
            .replace("a", a, &TRIANGLE, &[2, 1, 0])
            .expect("others");
        assert_eq!(waiting(&meshes), [(1, true, vec![2, 1, 0])]);
        // Broken triangles leave it as it was.
        let error = (meshes.replace("a", a, &TRIANGLE, &[0, 1, 3])).expect_err("broken");
        assert_eq!(error.code(), "renderer.bad_mesh");
        assert_eq!(waiting(&meshes), [(1, true, vec![2, 1, 0])]);
    }

    /// A mesh removed goes after the changes before it, once however many times, by its handle or
    /// its index; its handle isn't given out again, and a replacement of it is dropped, as its
    /// triangles are checked still.
    #[test]
    fn a_mesh_removed_stays_removed_in_order() {
        let mut meshes = Meshes::default();
        let a = meshes.add("a", &TRIANGLE, &[0, 1, 2]).expect("a");
        meshes.changes.clear();
        meshes
            .replace("a", a, &TRIANGLE, &[2, 1, 0])
            .expect("others");
        assert!(meshes.remove(a), "removed");
        assert!(!meshes.remove_index(a.index()), "removed already");
        let b = meshes.add("b", &TRIANGLE, &[0, 1, 2]).expect("b");
        assert_eq!((a.index, b.index), (1, 2));
        let order: Vec<(u32, bool)> = (meshes.changes.iter())
            .map(|change| match change {
                Change::Added(added) => (added.index, false),
                Change::Removed(index) => (*index, true),
            })
            .collect();
        assert_eq!(order, [(1, false), (1, true), (2, false)]);
        meshes.changes.clear();
        meshes
            .replace("a", a, &TRIANGLE, &[0, 1, 2])
            .expect("checked");
        assert!(meshes.changes.is_empty(), "dropped");
        let error = (meshes.replace("a", a, &TRIANGLE, &[0, 1, 3])).expect_err("broken");
        assert_eq!(error.code(), "renderer.bad_mesh");
    }

    #[test]
    #[should_panic(expected = "is one of another `Meshes`")]
    fn another_meshes_mesh_cant_be_removed() {
        Meshes::default().remove(Mesh::<Vec2>::new(5));
    }

    /// An asset waits for room when its bytes would take usage past the budget, however large,
    /// while something else is being copied; with nothing else copying, the oldest waiting goes
    /// ahead alone, so that nothing waits forever on a GPU past its budget already.
    #[test]
    fn an_asset_past_the_budget_waits_unless_nothing_else_is_copying() {
        assert!(!exceeds(10, 100, 90));
        assert!(exceeds(10, 100, 91));
        assert!(exceeds(0, 100, 101));
        assert!(!exceeds(0, 0, 0));
        assert!(exceeds(u64::MAX, 100, 1));
        assert!(!waits(10, 100, 90, true));
        assert!(waits(10, 100, 91, true));
        assert!(!waits(10, 100, 91, false));
        assert!(!waits(200, 100, 16, false), "past the budget already");
        assert!(waits(200, 100, 16, true));
    }

    /// A frame copies its budget while the transfer queue is at most a frame behind, and nothing
    /// once it's two or more behind.
    #[test]
    fn a_frame_copies_nothing_once_the_transfer_queue_falls_behind() {
        assert_eq!(budget_behind(0, 1 << 20), 1 << 20);
        assert_eq!(budget_behind(1, 1 << 20), 1 << 20);
        assert_eq!(budget_behind(2, 1 << 20), 0);
        assert_eq!(budget_behind(7, 1 << 20), 0);
    }

    /// Buffers or an image that don't fit the GPU's memory leave their asset waiting, warned once;
    /// any other failure ends the renderer.
    #[test]
    fn an_asset_that_doesnt_fit_waits_and_other_failures_propagate() {
        let mut warned = false;
        let no_room = || Error::new("gpu.out_of_memory", "vkCreateBuffer failed");
        assert_eq!(
            room_for(Err::<u32, _>(no_room()), "a", 16, &mut warned).ok(),
            Some(None)
        );
        assert!(warned);
        assert_eq!(room_for(Ok(5), "a", 16, &mut warned).ok(), Some(Some(5)));
        let lost = Error::new("gpu.device_lost", "vkCreateBuffer failed");
        let error = room_for(Err::<u32, _>(lost), "a", 16, &mut warned).expect_err("propagated");
        assert_eq!(error.code(), "gpu.device_lost");
    }

    #[test]
    fn uploads_keep_what_a_frame_left_ahead_of_what_they_take_next() {
        let mut meshes = Meshes::default();
        let mut shaders = crate::Shaders::default();
        let mut textures = crate::Textures::default();
        let mut materials = crate::Materials::default();
        let mut uploads = Uploads::default();
        uploads.take(&mut meshes, &mut shaders, &mut textures, &mut materials);
        // As a frame that presented nothing leaves them, and more are added before the next.
        meshes.add("a", &TRIANGLE, &[0, 1, 2]).unwrap();
        textures.add("a", [1, 1], &[255; 4], Mipmaps::None).unwrap();
        uploads.take(&mut meshes, &mut shaders, &mut textures, &mut materials);
        let meshes_held: Vec<u32> = (uploads.meshes.iter())
            .map(|change| match change {
                Change::Added(added) => added.index,
                Change::Removed(index) => *index,
            })
            .collect();
        // A texture's slot, then its image.
        let textures_held: Vec<u32> = (uploads.textures.iter())
            .map(|change| match change {
                texture::Change::Reserved { index, .. } => *index,
                texture::Change::Image(added) => added.index,
                texture::Change::Removed(index) => *index,
            })
            .collect();
        assert_eq!((meshes_held, textures_held), (vec![0, 1], vec![0, 0, 1, 1]));
        assert!(meshes.changes.is_empty() && textures.changes.is_empty());
    }

    #[test]
    fn levels_of_detail_are_whole_triangles_among_the_indices_with_errors_that_grow() {
        let mut meshes = Meshes::default();
        let level = |start, count, error| Lod {
            start,
            count,
            error,
        };
        let sphere = |levels: Vec<Lod>, center: Vec3, radius: f32| Lods {
            levels,
            center,
            radius,
        };
        let levels = |levels: Vec<Lod>| sphere(levels, Vec3::ZERO, 1.0);
        let bad = [
            ("no levels of detail", levels(vec![])),
            (
                "9 levels of detail, past the 8",
                levels(vec![level(0, 3, 0.0); MAX_LEVELS + 1]),
            ),
            ("past its 9 indices", levels(vec![level(6, 6, 0.0)])),
            ("past its 9 indices", levels(vec![level(u32::MAX, 3, 0.0)])),
            ("without triangles", levels(vec![level(3, 0, 0.0)])),
            ("not three for each", levels(vec![level(0, 4, 0.0)])),
            ("not a finite distance", levels(vec![level(0, 3, -1.0)])),
            ("not a finite distance", levels(vec![level(0, 3, f32::NAN)])),
            (
                "not a finite distance",
                levels(vec![level(0, 3, f32::INFINITY)]),
            ),
            (
                "less than the 0.5",
                levels(vec![level(0, 6, 0.5), level(6, 3, 0.25)]),
            ),
            ("centered", sphere(vec![level(0, 3, 0.0)], Vec3::NAN, 1.0)),
            (
                "centered",
                sphere(vec![level(0, 3, 0.0)], Vec3::INFINITY, 1.0),
            ),
            (
                "radius -1",
                sphere(vec![level(0, 3, 0.0)], Vec3::ZERO, -1.0),
            ),
            (
                "radius NaN",
                sphere(vec![level(0, 3, 0.0)], Vec3::ZERO, f32::NAN),
            ),
            (
                "radius inf",
                sphere(vec![level(0, 3, 0.0)], Vec3::ZERO, f32::INFINITY),
            ),
        ];
        for (problem, lods) in bad {
            let error = meshes
                .add_lods("broken", &TRIANGLE, &THREE, lods.clone())
                .expect_err(problem);
            assert_eq!(error.code(), "renderer.bad_mesh", "{lods:?}");
            let message = error.to_string();
            assert!(message.contains("\"broken\""), "{message}");
            assert!(message.contains(problem), "{message} for {problem}");
        }
        // Indices that no mesh has, though every level is among them.
        let error = (meshes.add_lods(
            "broken",
            &TRIANGLE,
            &[0, 1, 3],
            levels(vec![level(0, 3, 0.0)]),
        ))
        .expect_err("an index past the vertices");
        assert_eq!(error.code(), "renderer.bad_mesh");
        assert!(error.to_string().contains("past its 3 vertices"), "{error}");
        // Only the engine's quad has been added, whose handle the next follows.
        assert_eq!(meshes.count(), 1);
        assert_eq!(meshes.changes.len(), 1);

        // Levels may share indices, repeat an error, and not start at the first.
        let shared = levels(vec![level(0, 9, 0.0), level(3, 6, 0.1), level(3, 3, 0.1)]);
        let mesh = (meshes.add_lods("shared", &TRIANGLE, &THREE, shared.clone())).expect("a mesh");
        assert_eq!(mesh.index, 1);
        assert_eq!(adds(&meshes)[1].lods, shared);
        assert_eq!(adds(&meshes)[1].indices, THREE);
    }

    #[test]
    fn a_mesh_has_one_level_of_all_its_indices_unless_added_with_more_and_again_once_replaced() {
        let mut meshes = Meshes::default();
        let whole = |indices| Lods {
            levels: vec![Lod {
                start: 0,
                count: indices,
                error: 0.0,
            }],
            // No sphere, as `add` can't read the vertices' positions.
            center: Vec3::ZERO,
            radius: f32::INFINITY,
        };
        assert_eq!(adds(&meshes)[0].lods, whole(6), "the quad");
        let plain = meshes.add("plain", &TRIANGLE, &THREE).expect("a mesh");
        assert_eq!(adds(&meshes)[1].lods, whole(9));
        let finer = Lods {
            levels: vec![
                Lod {
                    start: 0,
                    count: 6,
                    error: 0.0,
                },
                Lod {
                    start: 6,
                    count: 3,
                    error: 0.5,
                },
            ],
            center: Vec3::X,
            radius: 2.0,
        };
        let with_levels = (meshes.add_lods("levels", &TRIANGLE, &THREE, finer.clone())).expect("a");
        assert_eq!(adds(&meshes)[2].lods, finer);
        // Replaced while it waits, and once a frame has taken it.
        meshes
            .replace("levels", with_levels, &TRIANGLE, &[0, 1, 2])
            .expect("others");
        assert_eq!(adds(&meshes)[2].lods, whole(3));
        meshes.changes.clear();
        meshes
            .replace("plain", plain, &TRIANGLE, &[0, 1, 2, 2, 1, 0])
            .expect("others");
        assert_eq!(adds(&meshes)[0].lods, whole(6));
        // Replaced with levels, which it takes as `add_lods` takes them, and checks alike, leaving
        // what waits as it was where they're refused.
        meshes
            .replace_lods("plain", plain, &TRIANGLE, &THREE, finer.clone())
            .expect("levels");
        assert_eq!(adds(&meshes)[0].lods, finer);
        let unbounded = Lods {
            radius: f32::INFINITY,
            ..finer.clone()
        };
        let refused = meshes.replace_lods("plain", plain, &TRIANGLE, &THREE, unbounded);
        assert_eq!(refused.expect_err("no sphere").code(), "renderer.bad_mesh");
        assert_eq!(adds(&meshes)[0].lods, finer);
    }

    /// What a viewpoint at the origin facing down -Z, or as `viewpoint` has it, sees of a view
    /// `size` pixels across and down.
    fn lod_view(viewpoint: Viewpoint, size: [u32; 2]) -> LodView {
        let aspect = size[0] as f32 / size[1] as f32;
        LodView::new(&viewpoint.view_projection(aspect), size)
    }

    fn looking(projection: Projection) -> Viewpoint {
        Viewpoint {
            projection,
            ..Viewpoint::default()
        }
    }

    #[test]
    fn perspective_picks_the_coarsest_level_whose_error_spans_a_pixel_where_the_sphere_is_nearest()
    {
        // A quarter turn from the view's bottom to its top, over 200 pixels: 100 a unit where W is
        // 1, and across too, over 320.
        let perspective = Projection::Perspective {
            fov: FRAC_PI_2,
            near: 0.1,
        };
        let view = lod_view(looking(perspective), [320, 200]);
        // The sphere's nearest point is a unit nearer than its center, so the second level's error
        // spans 0.05 * 100 / (depth - 1) pixels, and the third's 0.4 * 100 / (depth - 1).
        let lods = lods(&[0.0, 0.05, 0.4], 1.0);
        let level = |at: Vec3| view.level(&lods, &Mat4::from_translation(at));
        for (depth, expected) in [(1.5, 0), (5.9, 0), (6.1, 1), (40.9, 1), (41.1, 2), (1e6, 2)] {
            assert_eq!(level(Vec3::new(0.0, 0.0, -depth)), expected, "{depth} away");
            // Off to the side, as far from the center of the view as it is far, it's as far.
            let aside = Vec3::new(depth, -depth / 2.0, -depth);
            assert_eq!(level(aside), expected, "{depth} away and aside");
        }
        // Behind the camera, and so close that its sphere reaches the plane of the camera.
        assert_eq!(level(Vec3::new(0.0, 0.0, 1e3)), 0, "behind");
        assert_eq!(
            level(Vec3::new(0.0, 0.0, -1.0)),
            0,
            "the sphere reaches the plane"
        );

        // Turned and moved, the camera sees the same: the distance ahead is what counts.
        let rotation = Quat::from_rotation_y(1.0) * Quat::from_rotation_x(-0.4);
        let position = Vec3::new(3.0, -2.0, 5.0);
        let viewpoint = Viewpoint {
            position,
            rotation,
            projection: perspective,
        };
        let view = lod_view(viewpoint, [320, 200]);
        for (depth, expected) in [(5.9, 0), (6.1, 1), (40.9, 1), (41.1, 2)] {
            let at = position + rotation * Vec3::new(0.0, 0.0, -depth);
            let placed = view.level(&lods, &Mat4::from_translation(at));
            assert_eq!(placed, expected, "{depth} away from a turned camera");
        }
    }

    #[test]
    fn a_transform_scales_the_error_and_the_sphere_by_its_longest_axis() {
        let perspective = Projection::Perspective {
            fov: FRAC_PI_2,
            near: 0.1,
        };
        let view = lod_view(looking(perspective), [320, 200]);
        let lods = lods(&[0.0, 0.05], 1.0);
        let level = |scale: Vec3, depth: f32| {
            // Turned, so that the axes aren't the world's.
            let turn = Quat::from_rotation_z(0.7) * Quat::from_rotation_y(0.3);
            let at = Vec3::new(1.0, 2.0, -depth);
            let placed = Mat4::from_scale_rotation_translation(scale, turn, at);
            view.level(&lods, &placed)
        };
        // Doubled: the error spans 0.05 * 2 * 100 / (depth - 2) pixels.
        assert_eq!(level(Vec3::splat(2.0), 11.9), 0);
        assert_eq!(level(Vec3::splat(2.0), 12.1), 1);
        // Three times as long on one axis alone: the same, by 3 and (depth - 3), whichever axis.
        for scale in [
            Vec3::new(3.0, 1.0, 1.0),
            Vec3::new(1.0, 3.0, 2.0),
            Vec3::new(0.5, 1.0, 3.0),
        ] {
            assert_eq!(level(scale, 17.9), 0, "{scale}");
            assert_eq!(level(scale, 18.1), 1, "{scale}");
        }
    }

    #[test]
    fn the_sphere_is_round_its_center_not_the_meshs_origin() {
        let view = lod_view(
            looking(Projection::Perspective {
                fov: FRAC_PI_2,
                near: 0.1,
            }),
            [320, 200],
        );
        // 5 farther than the origin of the mesh, so that its sphere's nearest point is 4 farther
        // than the origin's depth, and the second level's error spans 0.05 * 100 / (depth + 4)
        // pixels: one at 1.
        let behind = Lods {
            center: Vec3::new(0.0, 0.0, -5.0),
            ..lods(&[0.0, 0.05], 1.0)
        };
        let level = |depth: f32| {
            let at = Mat4::from_translation(Vec3::new(0.0, 0.0, -depth));
            view.level(&behind, &at)
        };
        assert_eq!(level(0.9), 0);
        assert_eq!(level(1.1), 1);
        // Turned a half about Y, the center is 5 nearer than the origin instead, and the nearest
        // point 6: 0.05 * 100 / (depth - 6) is a pixel at 11.
        let turned = |depth: f32| {
            let at = Mat4::from_translation(Vec3::new(0.0, 0.0, -depth));
            view.level(&behind, &(at * Mat4::from_rotation_y(std::f32::consts::PI)))
        };
        assert_eq!(turned(10.9), 0);
        assert_eq!(turned(11.1), 1);
    }

    #[test]
    fn orthographic_levels_do_not_depend_on_distance() {
        // 4 units over 200 pixels: 50 a unit, and across too. The error spans 50 * error pixels
        // wherever the mesh is, and the sphere's radius doesn't reach any nearer.
        let orthographic = Projection::Orthographic {
            height: 4.0,
            near: -10.0,
            far: 10.0,
        };
        let view = lod_view(looking(orthographic), [200, 200]);
        let lods = lods(&[0.0, 0.01, 0.03], 5.0);
        for z in [-9.0, -1.0, 0.0, 3.0, 9.0] {
            let at = Mat4::from_translation(Vec3::new(1.0, -1.0, z));
            assert_eq!(view.level(&lods, &at), 1, "at {z}");
        }
        // Scaled, 0.01 spans a pixel at 2 and 0.03 at 0.66.
        let scaled = Mat4::from_scale(Vec3::splat(0.6));
        assert_eq!(view.level(&lods, &scaled), 2);
        let scaled = Mat4::from_scale(Vec3::splat(2.1));
        assert_eq!(view.level(&lods, &scaled), 0);

        // The canvas spans a pixel a unit.
        let canvas = LodView::new(&canvas_projection([400, 300]), [400, 300]);
        let lods = self::lods(&[0.0, 0.5, 2.0], 10.0);
        let level = |scale: f32| canvas.level(&lods, &Mat4::from_scale(Vec3::splat(scale)));
        assert_eq!([level(1.0), level(0.4), level(3.0)], [1, 2, 0]);
    }

    #[test]
    fn a_sphere_that_reaches_the_plane_of_the_camera_has_its_finest_level() {
        let view = lod_view(
            looking(Projection::Perspective {
                fov: FRAC_PI_2,
                near: 0.1,
            }),
            [320, 200],
        );
        // An error far under a pixel anywhere ahead of the sphere's nearest point.
        let lods = lods(&[0.0, 0.0005], 3.0);
        let level = |depth: f32| {
            let at = Mat4::from_translation(Vec3::new(0.0, 0.0, -depth));
            view.level(&lods, &at)
        };
        assert_eq!(level(-5.0), 0, "behind it");
        assert_eq!(level(0.0), 0, "in its plane");
        assert_eq!(level(2.9), 0, "across it");
        assert_eq!(level(3.0), 0, "touching it");
        assert_eq!(level(3.1), 1, "just ahead of it");
    }

    #[test]
    fn a_mesh_of_one_level_draws_its_indices_however_far() {
        let view = lod_view(
            looking(Projection::Perspective {
                fov: FRAC_PI_2,
                near: 0.1,
            }),
            [320, 200],
        );
        let whole = Lods::whole(9);
        for depth in [-1.0, 0.5, 10.0, 1e9] {
            let at = Mat4::from_translation(Vec3::new(0.0, 0.0, -depth));
            assert_eq!(view.level(&whole, &at), 0, "{depth} away");
        }
        // A transform that is no number leaves the finest level too.
        let broken = Mat4::from_scale(Vec3::NAN);
        assert_eq!(view.level(&lods(&[0.0, 1.0], 1.0), &broken), 0);
    }
}
