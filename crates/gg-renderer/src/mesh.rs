//! Meshes: the handles that the engine draws with, and the vertices and indices behind them in
//! GPU memory, drawn with their shaders and textures, games' and the engine's, in the game's view,
//! as its camera sees them.

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2};
use gg_core::{Error, Result};
use gg_gpu::{
    ColorImage, Device, Frame, GpuBuffer, GraphicsDesc, Pass, Pipeline, Texture as GpuTexture,
    TextureFormat,
};
use tracing::warn;

use crate::shader::{self, Shader};
use crate::texture::{self, Mipmaps, Texels};
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
    /// [`Shaders::add`](crate::Shaders::add) says.
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
        let index = self.count;
        self.count += 1;
        self.added.push(Added {
            index,
            replacement: false,
            name: name.to_owned(),
            vertices: bytemuck::cast_slice(vertices).to_vec(),
            vertex_size: size_of::<V>(),
            indices: indices.to_vec(),
        });
        Ok(Mesh::new(index))
    }

    /// Puts other triangles in place of `mesh`'s, which keeps its handle: `indices` and `vertices`
    /// as [`add`](Self::add) takes them, as many as they are, so that every draw of it draws them
    /// from the frame that copies them to the GPU on. It takes the place of any of that mesh's
    /// vertices and indices still waiting for a frame, as it was added or replaced. `name` names it
    /// in errors.
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
        let (vertices, indices) = (bytemuck::cast_slice(vertices).to_vec(), indices.to_vec());
        match (self.added.iter_mut()).find(|added| added.index == mesh.index) {
            Some(waiting) => {
                waiting.vertices = vertices;
                waiting.indices = indices;
            }
            None => self.added.push(Added {
                index: mesh.index,
                replacement: true,
                name: name.to_owned(),
                vertices,
                vertex_size: size_of::<V>(),
                indices,
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
    let bad = |problem: String| Error::new("renderer.bad_mesh", format!("mesh {name:?} {problem}"));
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
}

struct GpuShader {
    pipeline: Pipeline,
    /// The size of the vertices it reads.
    vertex_size: usize,
    /// The size of the parameters it takes.
    params_size: usize,
}

struct GpuMesh {
    vertices: GpuBuffer,
    indices: GpuBuffer,
    /// How many indices.
    count: u32,
    vertex_size: usize,
}

impl MeshPass {
    pub(crate) fn new(device: &Arc<Device>) -> Self {
        Self {
            device: device.clone(),
            shaders: Vec::new(),
            meshes: Vec::new(),
            textures: Vec::new(),
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
            let made = Pipeline::graphics(
                &self.device,
                &GraphicsDesc {
                    name: &added.name,
                    spirv: &added.spirv,
                    vertex: c"vertex_main",
                    fragment: c"fragment_main",
                    format: ColorImage::FORMAT.with_ids().with_depth(),
                    blend: added.blend,
                    depth_write: shader::writes_depth(added.blend),
                },
            );
            match self.shaders.get_mut(index) {
                Some(old) => match made {
                    Ok(pipeline) => old.pipeline = pipeline,
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
                count: added.indices.len() as u32,
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
            let mipmaps = added.texels.mipmaps();
            texture::add_mipmaps(&mut added.pixels, added.size, mipmaps);
            let mipmapped = mipmaps != Mipmaps::None;
            // A replacement keeps the format it was made with, as its `Textures` keeps its kind.
            if let Some(texture) = self.textures.get_mut(index) {
                frame.refill_texture(texture, added.size, mipmapped, &added.pixels)?;
                continue;
            }
            // In the slot of its handle's index, which shaders sample it by.
            let [width, height] = added.size;
            let format = match added.texels {
                Texels::Color(_) => TextureFormat::Rgba8Srgb,
                Texels::Data => TextureFormat::Rgba8Unorm,
            };
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
    /// `view_projection` takes the world to clip space, each run of them as one instanced draw, and
    /// copies each run's transforms, IDs and parameters into the frame for its shader. Returns what
    /// it drew.
    ///
    /// # Panics
    ///
    /// If a mesh drawn isn't one the pass has uploaded, a shader isn't one it has made a pipeline
    /// of, a mesh's vertices or a draw's parameters aren't the size its shader reads, a run has
    /// more instances than Vulkan's 32-bit count, or the pass isn't into an image of
    /// [`ColorImage::FORMAT`] with IDs and a depth image.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        view_projection: Mat4,
        view: [u32; 2],
        draws: &Draws,
    ) -> Result<DrawCounts> {
        let _zone = gg_core::zone!();
        let mut drawn = DrawCounts::default();
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
            let transforms = &draws.transforms[run.instances.clone()];
            let ids = &draws.ids[run.instances.clone()];
            let params = &draws.params[run.params.clone()];
            // A draw pairs a mesh and parameters with a shader for their types, but handles from
            // another `Meshes` or `Shaders` could pair them with one that reads past them.
            assert_eq!(
                (mesh.vertex_size, params.len()),
                (shader.vertex_size, shader.params_size * transforms.len()),
                "a mesh draws with a shader for its vertex type, and the parameters that shader \
                 takes, from the renderer's `Meshes` and `Shaders`"
            );
            let instances = u32::try_from(transforms.len())
                .expect("a run's instances fit the 32-bit count Vulkan takes");
            if bound != Some(run.shader) {
                pass.set_pipeline(&shader.pipeline);
                bound = Some(run.shader);
            }
            if faces != Some(run.faces) {
                pass.set_faces(run.faces);
                faces = Some(run.faces);
            }
            // Where the shader reads them during this frame, each array in an upload of its own,
            // so that each starts 16-byte aligned; a shader that takes no parameters gets 0.
            let transforms = pass.upload(bytemuck::cast_slice(transforms))?.address();
            let ids = pass.upload(bytemuck::cast_slice(ids))?.address();
            let params = if params.is_empty() {
                0
            } else {
                pass.upload(params)?.address()
            };
            let push = Push {
                clip_from_world: view_projection,
                vertices: mesh.vertices.address(),
                transforms,
                ids,
                params,
                view: view.map(|side| side as f32),
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.draw_indexed(&mesh.indices, 0, mesh.count, 0, instances);
            drawn.draws += 1;
            drawn.instances += run.instances.len();
            drawn.triangles += mesh.count as usize / 3 * run.instances.len();
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
    use super::*;

    const TRIANGLE: [[f32; 3]; 3] = [[0.0, -0.5, 0.0], [0.5, 0.5, 0.0], [-0.5, 0.5, 0.0]];

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
}
