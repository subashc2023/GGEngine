//! Meshes: the handles that the engine draws with, and the vertices and indices behind them in
//! GPU memory, drawn with their shaders and textures, games' and the engine's, in the game's view,
//! as its camera sees them.

use std::fmt;
use std::marker::PhantomData;
use std::ops::Range;
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_gpu::{
    ColorImage, Device, Frame, GpuBuffer, GraphicsDesc, Pass, Pipeline, Texture as GpuTexture,
};
use tracing::warn;

use crate::shader::{self, Shader};
use crate::texture;
use crate::{DrawCounts, Draws, Uploads, grid, sprite};

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

/// A mesh's levels of detail, and the sphere round its vertices that decides which a draw uses.
/// The levels go finest first, so level 0 is the mesh itself, usually with an error of 0, and each
/// has at least the error of the one before. They may share indices, as a chain of levels
/// simplified one from another does.
#[derive(Clone, Debug, PartialEq)]
pub struct Lods {
    /// The levels, finest first.
    pub levels: Vec<Lod>,
    /// The center of a sphere that holds every vertex, in the mesh's own space.
    pub center: Vec3,
    /// Its radius: how near the camera the mesh can reach, where it is largest on screen. One
    /// larger than the tightest only keeps finer levels in use at greater distances.
    pub radius: f32,
}

impl Lods {
    /// The levels of a mesh that has one, all of its `indices`.
    fn whole(indices: usize) -> Self {
        let level = Lod {
            start: 0,
            count: indices as u32,
            error: 0.0,
        };
        Self {
            levels: vec![level],
            center: Vec3::ZERO,
            radius: 0.0,
        }
    }
}

/// The meshes a renderer draws, the first of them the engine's quad, which sprites are drawn on.
/// It hands out each one's handle at once, and keeps its vertices and indices until
/// [`Uploads::take`] takes them, for a frame to copy to the GPU; a mesh replaced by others the same
/// way. A renderer draws the meshes of one `Meshes` only.
pub struct Meshes {
    /// Those added or replaced since they were last taken, in order.
    pub(crate) added: Vec<Added>,
    /// How many have been added.
    count: u32,
    /// The handle of the engine's grid mesh, which is added the first time it's asked for, once
    /// it has been.
    grid: Option<u32>,
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
    /// [`Shaders::add`](crate::Shaders::add) says. It has one level of detail, all its indices.
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
    /// kept in one buffer on the GPU.
    ///
    /// Fails with `renderer.bad_mesh` as `add` does over all of `indices`, and if `lods` has no
    /// levels; has a level that is empty, isn't whole triangles or reaches past `indices`; has an
    /// error that is negative, isn't finite, or is less than the level before's; has a center that
    /// isn't finite; or a radius that is negative or isn't finite.
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
        self.added.push(Added {
            index,
            replacement: false,
            name: name.to_owned(),
            vertices: bytemuck::cast_slice(vertices).to_vec(),
            vertex_size: size_of::<V>(),
            indices: indices.to_vec(),
            lods,
        });
        Mesh::new(index)
    }

    /// Puts other triangles in place of `mesh`'s, which keeps its handle: `indices` and `vertices`
    /// as [`add`](Self::add) takes them, as many as they are, so that every draw of it draws them
    /// from the frame that copies them to the GPU on. It takes the place of any of that mesh's
    /// vertices and indices still waiting for a frame, as it was added or replaced. `name` names it
    /// in errors. The mesh has one level of detail again, all its indices.
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
        assert!(
            mesh.index < self.count,
            "mesh {name:?} replaces one of another `Meshes`"
        );
        check(name, vertices, indices)?;
        let lods = Lods::whole(indices.len());
        let (vertices, indices) = (bytemuck::cast_slice(vertices).to_vec(), indices.to_vec());
        match (self.added.iter_mut()).find(|added| added.index == mesh.index) {
            Some(waiting) => {
                waiting.vertices = vertices;
                waiting.indices = indices;
                waiting.lods = lods;
            }
            None => self.added.push(Added {
                index: mesh.index,
                replacement: true,
                name: name.to_owned(),
                vertices,
                vertex_size: size_of::<V>(),
                indices,
                lods,
            }),
        }
        Ok(())
    }
}

/// Checks that `indices` are whole triangles of `vertices`, mesh `name`'s, as [`Meshes::add`]
/// says.
fn check<V>(name: &str, vertices: &[V], indices: &[u32]) -> Result<()> {
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
fn check_lods(name: &str, indices: &[u32], lods: &Lods) -> Result<()> {
    let bad = |problem: String| bad_mesh(name, problem);
    if lods.levels.is_empty() {
        return Err(bad("has no levels of detail".to_owned()));
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
fn bad_mesh(name: &str, problem: String) -> Error {
    Error::new("renderer.bad_mesh", format!("mesh {name:?} {problem}"))
}

impl Default for Meshes {
    fn default() -> Self {
        let mut meshes = Self {
            added: Vec::new(),
            count: 0,
            grid: None,
        };
        let quad = meshes
            .add("quad", &sprite::CORNERS, &sprite::INDICES)
            .expect("the quad is whole triangles of its own corners");
        assert_eq!(quad.index, sprite::QUAD.index, "the quad comes first");
        meshes
    }
}

/// Draws meshes with their shaders into the game's view, an image whose format is
/// [`ColorImage::FORMAT`], writing their IDs beside it and testing its depth image, and keeps on
/// the GPU the meshes' vertices and indices and the textures the shaders sample.
pub(crate) struct MeshPass {
    device: Arc<Device>,
    /// Each shader's pipeline, by its handle.
    shaders: Vec<GpuShader>,
    /// Each mesh's buffers, by its handle.
    meshes: Vec<GpuMesh>,
    /// Each texture, by its handle, which is its slot in the bindless set too.
    textures: Vec<GpuTexture>,
    /// Kept between frames, so that those that draw no more than the last allocate nothing.
    scratch: LodScratch,
}

struct GpuShader {
    pipeline: Pipeline,
    /// The size of the vertices it reads.
    vertex_size: usize,
    /// The size of the parameters it takes.
    params_size: usize,
    /// Whether the pipeline writes depth, as [`shader::writes_depth`] says.
    writes_depth: bool,
}

struct GpuMesh {
    vertices: GpuBuffer,
    /// Every level's indices.
    indices: GpuBuffer,
    /// Where each level is in `indices`, and the sphere that picks among them: a mesh added
    /// without levels has one, all its indices.
    lods: Lods,
    vertex_size: usize,
}

impl MeshPass {
    pub(crate) fn new(device: &Arc<Device>) -> Self {
        Self {
            device: device.clone(),
            shaders: Vec::new(),
            meshes: Vec::new(),
            textures: Vec::new(),
            scratch: LodScratch::default(),
        }
    }

    /// Makes the pipelines of the shaders in `uploads`, taking them. A shader that has a pipeline
    /// already, compiled again, gets a new one in place of the old, which retires until the GPU is
    /// done with it; one whose new pipeline can't be made warns, and keeps the old.
    ///
    /// # Panics
    ///
    /// If they were taken from another `Shaders` than the pass's shaders before them.
    pub(crate) fn add_shaders(&mut self, uploads: &mut Uploads) -> Result<()> {
        let _zone = gg_core::zone!();
        for added in uploads.shaders.drain(..) {
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
            if let Some(old) = self.shaders.get(index) {
                assert_eq!(
                    (old.vertex_size, old.params_size),
                    (added.vertex_size, added.params_size),
                    "shader {:?} is replaced by one for other types; a renderer draws with the \
                     shaders of one `Shaders`",
                    added.name
                );
            }
            let writes_depth = shader::writes_depth(added.blend);
            let made = Pipeline::graphics(
                &self.device,
                &GraphicsDesc {
                    name: &added.name,
                    spirv: &added.spirv,
                    vertex: c"vertex_main",
                    fragment: c"fragment_main",
                    format: ColorImage::FORMAT.with_ids().with_depth(),
                    blend: added.blend,
                    depth_write: writes_depth,
                },
            );
            match self.shaders.get_mut(index) {
                Some(old) => match made {
                    Ok(pipeline) => {
                        old.pipeline = pipeline;
                        old.writes_depth = writes_depth;
                    }
                    Err(error) => warn!(
                        code = error.code(),
                        shader = added.name,
                        "{error:#}; the shader keeps drawing as it did"
                    ),
                },
                None => self.shaders.push(GpuShader {
                    pipeline: made?,
                    vertex_size: added.vertex_size,
                    params_size: added.params_size,
                    writes_depth,
                }),
            }
        }
        Ok(())
    }

    /// Copies the meshes and textures in `uploads` to the GPU, taking them, which must happen
    /// before the frame's passes. A mesh replaced gets buffers anew, the old retiring until the GPU
    /// is done with them; a texture replaced, an image anew in its slot, once the GPU has finished
    /// the frames before, which may sample the old, as [`Frame::refill_texture`] says.
    ///
    /// # Panics
    ///
    /// If they were taken from another `Meshes` or `Textures` than the pass's meshes and textures
    /// before them.
    pub(crate) fn upload(&mut self, frame: &mut Frame<'_>, uploads: &mut Uploads) -> Result<()> {
        let _zone = gg_core::zone!();
        for added in uploads.meshes.drain(..) {
            let index = added.index as usize;
            // A mesh replaced is one the pass has; any other is the next its `Meshes` added.
            let follows = if added.replacement {
                index < self.meshes.len()
            } else {
                index == self.meshes.len()
            };
            assert!(follows, "a renderer draws the meshes of one `Meshes`");
            let indices: &[u8] = bytemuck::cast_slice(&added.indices);
            let buffer = |bytes: &[u8], what| {
                let name = format!("{} {what}", added.name);
                GpuBuffer::new(&self.device, bytes.len() as u64, &name)
            };
            let mut mesh = GpuMesh {
                vertices: buffer(&added.vertices, "vertices")?,
                indices: buffer(indices, "indices")?,
                lods: added.lods,
                vertex_size: added.vertex_size,
            };
            frame.fill_buffer(&mut mesh.vertices, &added.vertices)?;
            frame.fill_buffer(&mut mesh.indices, indices)?;
            match self.meshes.get_mut(index) {
                Some(old) => {
                    // Which the draws' sizes are checked against: its own type, since only the
                    // `Meshes` that added it replaces it.
                    assert_eq!(
                        old.vertex_size, mesh.vertex_size,
                        "mesh {:?} is replaced by vertices of another type; a renderer draws the \
                         meshes of one `Meshes`",
                        added.name
                    );
                    *old = mesh;
                }
                None => self.meshes.push(mesh),
            }
        }
        for mut added in uploads.textures.drain(..) {
            let index = added.index as usize;
            // A texture replaced is one the pass has; any other is the next its `Textures` added.
            let follows = if added.replacement {
                index < self.textures.len()
            } else {
                index == self.textures.len()
            };
            assert!(follows, "a renderer samples the textures of one `Textures`");
            texture::add_mipmaps(&mut added.pixels, added.size, added.texels.mipmaps());
            let (format, mipmapped) = (added.texels.format(), added.texels.mipmapped());
            // A replacement may be compressed where the image it replaces wasn't, or the other way
            // round, as its `Textures` keeps only its kind.
            if let Some(texture) = self.textures.get_mut(index) {
                frame.refill_texture(texture, format, added.size, mipmapped, &added.pixels)?;
                continue;
            }
            // In the slot of its handle's index, which shaders sample it by.
            let [width, height] = added.size;
            let mut texture = GpuTexture::placed(
                &self.device,
                added.index,
                width,
                height,
                format,
                mipmapped,
                &added.name,
            )?;
            frame.fill_texture(&mut texture, &added.pixels)?;
            self.textures.push(texture);
        }
        Ok(())
    }

    /// Draws `draws` over the whole of the pass's target, `view` pixels across and down, where
    /// `view_projection` takes the world to clip space, each run of them as one instanced draw, or,
    /// of a mesh with levels of detail, as one for each level its instances draw at, and copies
    /// each draw's transforms, IDs and parameters into the frame for its shader. Returns what it
    /// drew.
    ///
    /// # Panics
    ///
    /// If a mesh drawn isn't one the pass has uploaded, a shader isn't one it has made a pipeline
    /// of, a mesh's vertices or a draw's parameters aren't the size its shader reads, a run has
    /// more instances than Vulkan's 32-bit count, or the pass isn't into an image of
    /// [`ColorImage::FORMAT`] with IDs and a depth image.
    pub(crate) fn draw(
        &mut self,
        pass: &mut Pass<'_, '_>,
        view_projection: Mat4,
        view: [u32; 2],
        draws: &Draws,
    ) -> Result<DrawCounts> {
        let _zone = gg_core::zone!();
        let mut drawn = DrawCounts::default();
        let lod_view = LodView::new(&view_projection, view);
        // What's set, which a pipeline's binding leaves as it is.
        let (mut bound, mut faces) = (None, None);
        for run in &draws.runs {
            let mesh = self
                .meshes
                .get(run.mesh as usize)
                .expect("a mesh of the renderer's `Meshes`, which the frame uploaded");
            let shader = self
                .shaders
                .get(run.shader as usize)
                .expect("a shader of the renderer's `Shaders`");
            let instances = Instances {
                transforms: &draws.transforms[run.instances.clone()],
                ids: &draws.ids[run.instances.clone()],
                params: &draws.params[run.params.clone()],
                params_size: shader.params_size,
            };
            // A draw pairs a mesh and parameters with a shader for their types, but handles from
            // another `Meshes` or `Shaders` could pair them with one that reads past them.
            assert_eq!(
                (mesh.vertex_size, instances.params.len()),
                (shader.vertex_size, shader.params_size * instances.len()),
                "a mesh draws with a shader for its vertex type, and the parameters that shader \
                 takes, from the renderer's `Meshes` and `Shaders`"
            );
            if bound != Some(run.shader) {
                pass.set_pipeline(&shader.pipeline);
                bound = Some(run.shader);
            }
            if faces != Some(run.faces) {
                pass.set_faces(run.faces);
                faces = Some(run.faces);
            }
            let batch = Batch {
                mesh,
                push: Push {
                    clip_from_world: view_projection,
                    vertices: mesh.vertices.address(),
                    // Where each draw's instances are, which it sets.
                    transforms: 0,
                    ids: 0,
                    params: 0,
                    view: view.map(|side| side as f32),
                },
                opaque: shader.writes_depth,
            };
            drawn += match mesh.lods.levels.as_slice() {
                [level] => batch.draw(pass, *level, instances)?,
                _ => batch.draw_levels(pass, &mut self.scratch, &lod_view, instances)?,
            };
        }
        Ok(drawn)
    }
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

/// A run's instances, or some of them: each one's transform, ID and parameters, in the same order.
#[derive(Clone, Copy)]
struct Instances<'a> {
    transforms: &'a [Mat4],
    ids: &'a [[u32; 2]],
    /// Every instance's parameters, one after another.
    params: &'a [u8],
    /// The size of one instance's parameters.
    params_size: usize,
}

impl Instances<'_> {
    fn len(&self) -> usize {
        self.transforms.len()
    }

    /// Those from `range`.
    fn range(&self, range: Range<usize>) -> Instances<'_> {
        let params = range.start * self.params_size..range.end * self.params_size;
        Instances {
            transforms: &self.transforms[range.clone()],
            ids: &self.ids[range],
            params: &self.params[params],
            params_size: self.params_size,
        }
    }
}

/// Memory for drawing instances of a mesh with levels of detail, which a pass keeps between draws
/// and frames: the level each instance of a run draws at, and the instances of one level gathered
/// where the run's own are apart. Each holds the largest run it has seen, so that a frame that
/// draws no more than the last allocates nothing, wherever its instances move between levels.
#[derive(Default)]
struct LodScratch {
    levels: Vec<usize>,
    transforms: Vec<Mat4>,
    ids: Vec<[u32; 2]>,
    params: Vec<u8>,
}

/// A run of instances of one mesh, with what its draws share.
struct Batch<'a> {
    mesh: &'a GpuMesh,
    /// What the pass pushes, but for the addresses of the instances, which each draw sets.
    push: Push,
    /// Whether the shader writes depth.
    opaque: bool,
}

impl Batch<'_> {
    /// Draws `instances` of the mesh's `level` as one instanced draw, copying their transforms, IDs
    /// and parameters into the frame. Returns what it drew.
    fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        level: Lod,
        instances: Instances<'_>,
    ) -> Result<DrawCounts> {
        let count = u32::try_from(instances.len())
            .expect("a run's instances fit the 32-bit count Vulkan takes");
        // Where the shader reads them during this frame, each array in an upload of its own, so
        // that each starts 16-byte aligned; a shader that takes no parameters gets 0.
        let transforms = pass
            .upload(bytemuck::cast_slice(instances.transforms))?
            .address();
        let ids = pass.upload(bytemuck::cast_slice(instances.ids))?.address();
        let params = if instances.params.is_empty() {
            0
        } else {
            pass.upload(instances.params)?.address()
        };
        let push = Push {
            transforms,
            ids,
            params,
            ..self.push
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw_indexed(&self.mesh.indices, level.start, level.count, 0, count);
        Ok(DrawCounts {
            draws: 1,
            instances: instances.len(),
            triangles: level.count as usize / 3 * instances.len(),
        })
    }

    /// Draws `instances` of a mesh with several levels of detail, each at the level `view` picks
    /// for it: one draw for each level in use if the shader writes depth, which hides what's
    /// behind whatever order the instances draw in, so that they join wherever they are in the
    /// run, each level's keeping their order; otherwise one for each stretch of instances in a
    /// row that use the same level, as what blends draws in order.
    fn draw_levels(
        &self,
        pass: &mut Pass<'_, '_>,
        scratch: &mut LodScratch,
        view: &LodView,
        instances: Instances<'_>,
    ) -> Result<DrawCounts> {
        let lods = &self.mesh.lods;
        let LodScratch {
            levels,
            transforms,
            ids,
            params,
        } = scratch;
        levels.clear();
        levels.extend((instances.transforms.iter()).map(|transform| view.level(lods, transform)));
        let mut drawn = DrawCounts::default();
        if !self.opaque {
            // A stretch's instances are already together in the run's arrays.
            let mut start = 0;
            for same in levels.chunk_by(|a, b| a == b) {
                let end = start + same.len();
                drawn += self.draw(pass, lods.levels[same[0]], instances.range(start..end))?;
                start = end;
            }
            return Ok(drawn);
        }
        let (finest, coarsest) = (levels.iter())
            .fold((usize::MAX, 0), |(finest, coarsest), &level| {
                (finest.min(level), coarsest.max(level))
            });
        if finest == coarsest {
            return self.draw(pass, lods.levels[finest], instances);
        }
        // A pass over the run for each level from the finest in use to the coarsest, which is cheap
        // while a mesh has a handful of levels. With many, a counting sort would gather them all
        // in one pass.
        for level in finest..=coarsest {
            transforms.clear();
            ids.clear();
            params.clear();
            transforms.reserve(instances.len());
            ids.reserve(instances.len());
            params.reserve(instances.params.len());
            let at = (levels.iter().enumerate()).filter(|&(_, &used)| used == level);
            for (i, _) in at {
                let one = instances.range(i..i + 1);
                transforms.extend_from_slice(one.transforms);
                ids.extend_from_slice(one.ids);
                params.extend_from_slice(one.params);
            }
            if transforms.is_empty() {
                continue;
            }
            let gathered = Instances {
                transforms,
                ids,
                params,
                params_size: instances.params_size,
            };
            drawn += self.draw(pass, lods.levels[level], gathered)?;
        }
        Ok(drawn)
    }
}

/// What the mesh pass pushes before each draw, which a mesh shader's push constants read the
/// start of, as [`Shaders::add`](crate::Shaders::add) describes, and which the engine's mesh
/// shaders declare as `Push` in `shaders/mesh.slang`. The matrix goes first, so that every offset
/// is the same under every layout Slang gives push constants; the IDs before the parameters, so
/// that a shader that takes none still ends its struct after the last member it reads; and the
/// view's size last, so that it moves none of the offsets games' shaders declare before it,
/// though a shader that takes no parameters can't read it.
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
            let added = meshes.added.iter();
            added
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
        meshes.added.clear();
        meshes
            .replace("a", a, &TRIANGLE, &[2, 1, 0])
            .expect("others");
        assert_eq!(waiting(&meshes), [(1, true, vec![2, 1, 0])]);
        // Broken triangles leave it as it was.
        let error = (meshes.replace("a", a, &TRIANGLE, &[0, 1, 3])).expect_err("broken");
        assert_eq!(error.code(), "renderer.bad_mesh");
        assert_eq!(waiting(&meshes), [(1, true, vec![2, 1, 0])]);
    }

    #[test]
    fn uploads_keep_what_a_frame_left_ahead_of_what_they_take_next() {
        let mut meshes = Meshes::default();
        let mut shaders = crate::Shaders::default();
        let mut textures = crate::Textures::default();
        let mut uploads = Uploads::default();
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        // As a frame that presented nothing leaves them, and more are added before the next.
        meshes.add("a", &TRIANGLE, &[0, 1, 2]).unwrap();
        textures.add("a", [1, 1], &[255; 4], Mipmaps::None).unwrap();
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let meshes_held: Vec<u32> = uploads.meshes.iter().map(|added| added.index).collect();
        let textures_held: Vec<u32> = uploads.textures.iter().map(|added| added.index).collect();
        assert_eq!((meshes_held, textures_held), (vec![0, 1], vec![0, 1]));
        assert!(meshes.added.is_empty() && textures.added.is_empty());
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
        assert_eq!(meshes.added.len(), 1);

        // Levels may share indices, repeat an error, and not start at the first.
        let shared = levels(vec![level(0, 9, 0.0), level(3, 6, 0.1), level(3, 3, 0.1)]);
        let mesh = (meshes.add_lods("shared", &TRIANGLE, &THREE, shared.clone())).expect("a mesh");
        assert_eq!(mesh.index, 1);
        assert_eq!(meshes.added[1].lods, shared);
        assert_eq!(meshes.added[1].indices, THREE);
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
            center: Vec3::ZERO,
            radius: 0.0,
        };
        assert_eq!(meshes.added[0].lods, whole(6), "the quad");
        let plain = meshes.add("plain", &TRIANGLE, &THREE).expect("a mesh");
        assert_eq!(meshes.added[1].lods, whole(9));
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
        assert_eq!(meshes.added[2].lods, finer);
        // Replaced while it waits, and once a frame has taken it.
        meshes
            .replace("levels", with_levels, &TRIANGLE, &[0, 1, 2])
            .expect("others");
        assert_eq!(meshes.added[2].lods, whole(3));
        meshes.added.clear();
        meshes
            .replace("plain", plain, &TRIANGLE, &[0, 1, 2, 2, 1, 0])
            .expect("others");
        assert_eq!(meshes.added[0].lods, whole(6));
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
