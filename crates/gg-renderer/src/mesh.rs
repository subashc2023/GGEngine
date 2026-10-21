//! Meshes: the handles that the engine draws with, and the vertices and indices behind them in
//! GPU memory, drawn with games' shaders in the game's view, as its camera sees them.

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::Mat4;
use gg_core::{Error, Result};
use gg_gpu::{Blend, Device, Format, Frame, GpuBuffer, GraphicsDesc, Pass, Pipeline};

use crate::shader::Shaders;
use crate::{Draw, Viewport};

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

impl<V> fmt::Debug for Mesh<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Mesh").field(&self.index).finish()
    }
}

/// The meshes a renderer draws. It hands out each one's handle at once, and keeps its vertices
/// and indices until the next frame that renders copies them to the GPU. A renderer draws the
/// meshes of one `Meshes` only.
#[derive(Default)]
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
        Ok(Mesh {
            index,
            vertex: PhantomData,
        })
    }
}

/// Draws meshes with games' shaders, and keeps the meshes' vertices and indices on the GPU.
pub(crate) struct MeshPass {
    device: Arc<Device>,
    /// The format of the targets it draws into.
    format: Format,
    /// Each shader's pipeline, and the size of the vertices it reads, by its handle.
    pipelines: Vec<(Pipeline, usize)>,
    /// Each mesh's buffers, by its handle.
    meshes: Vec<GpuMesh>,
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
            pipelines: Vec::new(),
            meshes: Vec::new(),
        }
    }

    /// Makes the pipelines of the shaders added to `shaders` since the pass last took them.
    ///
    /// # Panics
    ///
    /// If `shaders` isn't the one the pass has been taking shaders from.
    pub(crate) fn add_shaders(&mut self, shaders: &mut Shaders) -> Result<()> {
        for added in shaders.added.drain(..) {
            assert_eq!(
                added.index as usize,
                self.pipelines.len(),
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
                    blend: Blend::Opaque,
                },
            )?;
            self.pipelines.push((pipeline, added.vertex_size));
        }
        Ok(())
    }

    /// Copies the meshes added to `meshes` since a frame last took them to the GPU, which must
    /// happen before the frame's passes.
    ///
    /// # Panics
    ///
    /// If `meshes` isn't the one the pass has been taking meshes from.
    pub(crate) fn upload(&mut self, frame: &mut Frame<'_>, meshes: &mut Meshes) -> Result<()> {
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
        Ok(())
    }

    /// Draws `draws` in order, each over those before it, in `viewport` of the pass's target,
    /// where `view_projection` takes the world to clip space. A viewport without an area draws
    /// nothing.
    ///
    /// # Panics
    ///
    /// If a mesh drawn isn't one the pass has uploaded, a shader isn't one it has made a pipeline
    /// of, a mesh's vertices aren't the size its shader reads, or the target's format isn't the
    /// pass's.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        viewport: Viewport,
        view_projection: Mat4,
        draws: &[Draw],
    ) {
        if draws.is_empty() || viewport.size.contains(&0) {
            return;
        }
        pass.set_viewport(viewport.origin, viewport.size);
        let mut bound = None;
        for draw in draws {
            let mesh = self
                .meshes
                .get(draw.mesh as usize)
                .expect("a mesh of the renderer's `Meshes`, which the frame uploaded");
            let (pipeline, vertex_size) = self
                .pipelines
                .get(draw.shader as usize)
                .expect("a shader of the renderer's `Shaders`");
            // A `Draw` pairs a mesh with a shader for its vertex type, but handles from another
            // `Meshes` or `Shaders` could pair it with one that reads past its vertices.
            assert_eq!(
                mesh.vertex_size, *vertex_size,
                "a mesh draws with a shader for its vertex type, from the renderer's `Meshes` and \
                 `Shaders`"
            );
            if bound != Some(draw.shader) {
                pass.set_pipeline(pipeline);
                bound = Some(draw.shader);
            }
            // The shader's `Push`: the matrix from the mesh's space to clip space, column-major,
            // then the vertices' address. The matrix goes first, so that both offsets are the
            // same under every layout Slang gives push constants.
            let clip_from_mesh = view_projection * draw.transform;
            let mut push = [0u8; 72];
            push[0..64].copy_from_slice(bytemuck::cast_slice(&clip_from_mesh.to_cols_array()));
            push[64..72].copy_from_slice(&mesh.vertices.address().to_ne_bytes());
            pass.push_constants(&push);
            pass.draw_indexed(&mesh.indices, 0, mesh.count, 0);
        }
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
        assert_eq!(add(&[0, 1, 2]).ok(), Some(0));
        assert_eq!(add(&[2, 1, 0]).ok(), Some(1));
    }
}
