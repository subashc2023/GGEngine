//! Meshes: the handles that the engine draws with, and the vertices and indices behind them in
//! GPU memory, drawn with their shaders and textures, games' and the engine's, in the game's view,
//! as its camera sees them.

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::Mat4;
use gg_core::{Error, Result};
use gg_gpu::{
    Device, Format, Frame, GpuBuffer, GraphicsDesc, Pass, Pipeline, Texture as GpuTexture,
    TextureFormat,
};

use crate::shader::Shaders;
use crate::texture::Textures;
use crate::{DrawCounts, Draws, Viewport, sprite};

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

/// The meshes a renderer draws, the first of them the engine's quad, which sprites are drawn on.
/// It hands out each one's handle at once, and keeps its vertices and indices until the next frame
/// that renders copies them to the GPU. A renderer draws the meshes of one `Meshes` only.
pub struct Meshes {
    /// Those added since a frame last took them, in order.
    added: Vec<Added>,
    /// How many have been added.
    count: u32,
}

struct Added {
    index: u32,
    name: String,
    vertices: Vec<u8>,
    vertex_size: usize,
    indices: Vec<u32>,
}

impl Meshes {
    /// Adds a mesh of triangles, three of `indices` each, which index `vertices`. `name` names it
    /// in errors and GPU captures. It draws with shaders for `V`, which read its vertices as
    /// [`Shaders::add`] says.
    ///
    /// Fails with `renderer.bad_mesh` if it has no triangles, indices left over from whole
    /// triangles, or an index past its vertices.
    pub fn add<V: NoUninit>(
        &mut self,
        name: &str,
        vertices: &[V],
        indices: &[u32],
    ) -> Result<Mesh<V>> {
        // Vulkan's buffers hold at least a byte.
        const { assert!(size_of::<V>() > 0, "a vertex type has a size") };
        let bad =
            |problem: String| Error::new("renderer.bad_mesh", format!("mesh {name:?} {problem}"));
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
        let index = self.count;
        self.count += 1;
        self.added.push(Added {
            index,
            name: name.to_owned(),
            vertices: bytemuck::cast_slice(vertices).to_vec(),
            vertex_size: size_of::<V>(),
            indices: indices.to_vec(),
        });
        Ok(Mesh::new(index))
    }
}

impl Default for Meshes {
    fn default() -> Self {
        let mut meshes = Self {
            added: Vec::new(),
            count: 0,
        };
        let quad = meshes
            .add("quad", &sprite::CORNERS, &sprite::INDICES)
            .expect("the quad is whole triangles of its own corners");
        assert_eq!(quad.index, sprite::QUAD.index, "the quad comes first");
        meshes
    }
}

/// Draws meshes with their shaders, and keeps on the GPU the meshes' vertices and indices and the
/// textures the shaders sample.
pub(crate) struct MeshPass {
    device: Arc<Device>,
    /// The format of the targets it draws into.
    format: Format,
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
    /// A pass that draws into targets of `format`.
    pub(crate) fn new(device: &Arc<Device>, format: Format) -> Self {
        Self {
            device: device.clone(),
            format,
            shaders: Vec::new(),
            meshes: Vec::new(),
            textures: Vec::new(),
        }
    }

    /// Makes the pipelines of the shaders added to `shaders` since the pass last took them.
    ///
    /// # Panics
    ///
    /// If `shaders` isn't the one the pass has been taking shaders from.
    pub(crate) fn add_shaders(&mut self, shaders: &mut Shaders) -> Result<()> {
        let _zone = gg_core::zone!();
        for added in shaders.added.drain(..) {
            assert_eq!(
                added.index as usize,
                self.shaders.len(),
                "a renderer draws with the shaders of one `Shaders`"
            );
            let pipeline = Pipeline::graphics(
                &self.device,
                &GraphicsDesc {
                    name: &added.name,
                    spirv: &added.spirv,
                    vertex: c"vertex_main",
                    fragment: c"fragment_main",
                    format: self.format,
                    blend: added.blend,
                },
            )?;
            self.shaders.push(GpuShader {
                pipeline,
                vertex_size: added.vertex_size,
                params_size: added.params_size,
            });
        }
        Ok(())
    }

    /// Copies the meshes and textures added to `meshes` and `textures` since a frame last took them
    /// to the GPU, which must happen before the frame's passes.
    ///
    /// # Panics
    ///
    /// If `meshes` or `textures` isn't the one the pass has been taking them from.
    pub(crate) fn upload(
        &mut self,
        frame: &mut Frame<'_>,
        meshes: &mut Meshes,
        textures: &mut Textures,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
        for added in meshes.added.drain(..) {
            assert_eq!(
                added.index as usize,
                self.meshes.len(),
                "a renderer draws the meshes of one `Meshes`"
            );
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
            frame.write_buffer(&mut mesh.vertices, 0, &added.vertices)?;
            frame.write_buffer(&mut mesh.indices, 0, indices)?;
            self.meshes.push(mesh);
        }
        for added in textures.added.drain(..) {
            assert_eq!(
                added.index as usize,
                self.textures.len(),
                "a renderer samples the textures of one `Textures`"
            );
            // In the slot of its handle's index, which shaders sample it by.
            let [width, height] = added.size;
            let mut texture = GpuTexture::placed(
                &self.device,
                added.index,
                width,
                height,
                TextureFormat::Rgba8Srgb,
                &added.name,
            )?;
            frame.write_texture(&mut texture, [0, 0], added.size, &added.pixels)?;
            self.textures.push(texture);
        }
        Ok(())
    }

    /// Draws `draws` in `viewport` of the pass's target, where `view_projection` takes the world
    /// to clip space, each run of them as one instanced draw, and copies each run's transforms and
    /// parameters into the frame for its shader. A viewport without an area draws nothing. Returns
    /// what it drew.
    ///
    /// # Panics
    ///
    /// If a mesh drawn isn't one the pass has uploaded, a shader isn't one it has made a pipeline
    /// of, a mesh's vertices or a draw's parameters aren't the size its shader reads, a run has
    /// more instances than Vulkan's 32-bit count, or the target's format isn't the pass's.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        viewport: Viewport,
        view_projection: Mat4,
        draws: &Draws,
    ) -> Result<DrawCounts> {
        let _zone = gg_core::zone!();
        let mut drawn = DrawCounts::default();
        if draws.is_empty() || viewport.size.contains(&0) {
            return Ok(drawn);
        }
        pass.set_viewport(viewport.origin, viewport.size);
        let mut bound = None;
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
            // Where the shader reads them during this frame, each array in an upload of its own,
            // so that each starts 16-byte aligned; a shader that takes no parameters gets 0.
            let transforms = pass.upload(bytemuck::cast_slice(transforms))?.address();
            let params = if params.is_empty() {
                0
            } else {
                pass.upload(params)?.address()
            };
            // The shader's `Push`: the matrix from the world to clip space, column-major, then the
            // addresses of the vertices, the instances' transforms and their parameters. The
            // matrix goes first, so that every offset is the same under every layout Slang gives
            // push constants.
            let mut push = [0u8; 88];
            push[0..64].copy_from_slice(bytemuck::bytes_of(&view_projection));
            push[64..72].copy_from_slice(&mesh.vertices.address().to_ne_bytes());
            push[72..80].copy_from_slice(&transforms.to_ne_bytes());
            push[80..88].copy_from_slice(&params.to_ne_bytes());
            pass.push_constants(&push);
            pass.draw_indexed(&mesh.indices, 0, mesh.count, 0, instances);
            drawn.draws += 1;
            drawn.instances += run.instances.len();
        }
        Ok(drawn)
    }
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
}
