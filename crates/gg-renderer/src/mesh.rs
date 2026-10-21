//! Meshes: the handles that the engine draws with, and the vertices and indices behind them in
//! GPU memory, drawn in the game's view.

use std::sync::Arc;

use gg_core::{Error, Result};
use gg_gpu::{Blend, Device, Format, Frame, GpuBuffer, GraphicsDesc, Pass, Pipeline};

use crate::Viewport;

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mesh.spv"));

/// A corner of a mesh's triangles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Until there are cameras, in Vulkan's clip space, which spans the game's view: x from -1
    /// at the left to 1 at the right, y from -1 at the top to 1 at the bottom, and z from 0 to 1.
    pub position: [f32; 3],
    /// Linear RGBA, which a triangle blends between its corners.
    pub color: [f32; 4],
}

/// A mesh that [`Meshes::add`] handed out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mesh(u32);

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
    mesh: Mesh,
    name: String,
    /// Seven words a vertex: its position, then its color.
    vertices: Vec<f32>,
    indices: Vec<u32>,
}

impl Meshes {
    /// Adds a mesh of triangles, three of `indices` each, which index `vertices`. `name` names it
    /// in errors and GPU captures.
    ///
    /// Fails with `renderer.bad_mesh` if it has no triangles, indices left over from whole
    /// triangles, or an index past its vertices.
    pub fn add(&mut self, name: &str, vertices: &[Vertex], indices: &[u32]) -> Result<Mesh> {
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
        let mesh = Mesh(self.count);
        self.count += 1;
        let vertices = vertices
            .iter()
            .flat_map(|vertex| {
                let ([x, y, z], [r, g, b, a]) = (vertex.position, vertex.color);
                [x, y, z, r, g, b, a]
            })
            .collect();
        self.added.push(Added {
            mesh,
            name: name.to_owned(),
            vertices,
            indices: indices.to_vec(),
        });
        Ok(mesh)
    }
}

/// Draws meshes, and keeps their vertices and indices on the GPU.
pub(crate) struct MeshPass {
    device: Arc<Device>,
    /// The format of the target it draws into.
    format: Format,
    pipeline: Pipeline,
    /// Each mesh's buffers, by its handle.
    meshes: Vec<GpuMesh>,
}

struct GpuMesh {
    vertices: GpuBuffer,
    indices: GpuBuffer,
    /// How many indices.
    count: u32,
}

impl MeshPass {
    /// A pass that draws into targets of `format`.
    pub(crate) fn new(device: &Arc<Device>, format: Format) -> Result<Self> {
        let pipeline = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: "mesh",
                spirv: SPIRV,
                vertex: c"vertex_main",
                fragment: c"fragment_main",
                format,
                blend: Blend::Opaque,
            },
        )?;
        Ok(Self {
            device: device.clone(),
            format,
            pipeline,
            meshes: Vec::new(),
        })
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
                added.mesh.0 as usize,
                self.meshes.len(),
                "a renderer draws the meshes of one `Meshes`"
            );
            let vertices: &[u8] = bytemuck::cast_slice(&added.vertices);
            let indices: &[u8] = bytemuck::cast_slice(&added.indices);
            let buffer = |bytes: &[u8], what| {
                let name = format!("{} {what}", added.name);
                GpuBuffer::new(&self.device, bytes.len() as u64, &name)
            };
            let mut mesh = GpuMesh {
                vertices: buffer(vertices, "vertices")?,
                indices: buffer(indices, "indices")?,
                count: added.indices.len() as u32,
            };
            frame.write_buffer(&mut mesh.vertices, 0, vertices)?;
            frame.write_buffer(&mut mesh.indices, 0, indices)?;
            self.meshes.push(mesh);
        }
        Ok(())
    }

    /// Draws `draws` in order, each over those before it, in `viewport` of the pass's target. A
    /// viewport without an area draws nothing.
    ///
    /// # Panics
    ///
    /// If a mesh drawn isn't one the pass has uploaded, or the target's format isn't the pass's.
    pub(crate) fn draw(&self, pass: &mut Pass<'_, '_>, viewport: Viewport, draws: &[Mesh]) {
        if draws.is_empty() || viewport.size.contains(&0) {
            return;
        }
        assert_eq!(
            pass.format(),
            self.format,
            "meshes draw into the main target"
        );
        pass.set_viewport(viewport.origin, viewport.size);
        pass.set_pipeline(&self.pipeline);
        for mesh in draws {
            let mesh = self
                .meshes
                .get(mesh.0 as usize)
                .expect("a mesh of the renderer's `Meshes`, which the frame uploaded");
            // The shader's `Push`: the vertices' address.
            pass.push_constants(&mesh.vertices.address().to_ne_bytes());
            pass.draw_indexed(&mesh.indices, 0, mesh.count, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIANGLE: [Vertex; 3] = [
        Vertex {
            position: [0.0, -0.5, 0.0],
            color: [1.0, 0.0, 0.0, 1.0],
        },
        Vertex {
            position: [0.5, 0.5, 0.0],
            color: [0.0, 1.0, 0.0, 1.0],
        },
        Vertex {
            position: [-0.5, 0.5, 0.0],
            color: [0.0, 0.0, 1.0, 1.0],
        },
    ];

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
        assert_eq!(meshes.add("a", &TRIANGLE, &[0, 1, 2]).ok(), Some(Mesh(0)));
        assert_eq!(meshes.add("b", &TRIANGLE, &[2, 1, 0]).ok(), Some(Mesh(1)));
    }
}
