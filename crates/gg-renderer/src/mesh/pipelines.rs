//! The pipelines each shader draws with, made as it's added or compiled again.

use std::ffi::{CStr, CString};
use std::sync::Arc;

use gg_core::Result;
use gg_gpu::{Blend, Device, Format, GraphicsDesc, Pipeline};
use tracing::warn;

use super::{DepthOnly, GpuShader, MeshPass};
use crate::{Uploads, shader, shadow};

/// A shader's geometry, which its pipelines draw with: its module, named `name`, the module of its
/// shadows if they're cut out, as [`shader::Added::shadow`] says, and of its normals, for a model
/// shader, as [`shader::Meshlets`] says; and its geometry's entry point in each, or in `mesh`, a
/// mesh shader's module apart from them, as [`GraphicsDesc::mesh`] says.
struct Geometry<'a> {
    name: &'a str,
    spirv: &'a [u8],
    shadow: Option<&'a [u8]>,
    normals: Option<&'a [u8]>,
    entry: &'a CStr,
    mesh: Option<&'a [u8]>,
}

impl Geometry<'_> {
    /// Its pipeline, into the game's view, with `fragment_main`, blending as `blend` says and
    /// writing the IDs of what it draws if `ids` says so, as [`shader::Added::ids`] says; and, if
    /// that writes depth, what draws its depth alone, by its geometry and, if its shadows are cut
    /// out, `shadow_main`, and its normals, if it has a module of them, by `normal_main`.
    fn pipelines(
        &self,
        device: &Arc<Device>,
        blend: Blend,
        ids: bool,
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
                ids_write: ids,
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
                    ids_write: false,
                    depth_bias: if shadow { shadow::DEPTH_BIAS } else { 0.0 },
                    depth_clamp: shadow,
                },
            )
        };
        // Its normals, into an image of high dynamic range's format, tested against the depth drawn
        // alone and leaving it as it is, so that what's nearest there alone draws.
        let normals = |spirv| {
            Pipeline::graphics(
                device,
                &GraphicsDesc {
                    name: &format!("{} normals", self.name),
                    spirv,
                    vertex: self.entry,
                    mesh: self.mesh,
                    fragment: Some(c"normal_main"),
                    format: Format::HDR.with_depth(),
                    blend: Blend::Opaque,
                    depth_write: false,
                    ids_write: false,
                    depth_bias: 0.0,
                    depth_clamp: false,
                },
            )
        };
        let depth_only = writes_depth.then(|| {
            Ok(DepthOnly {
                shadow: depth_only(&format!("{} shadow", self.name), true)?,
                view: depth_only(&format!("{} depth", self.name), false)?,
                normals: self.normals.map(normals).transpose()?,
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
            ids_write: true,
            depth_bias: 0.0,
            depth_clamp: false,
        },
    )
}

impl MeshPass {
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
            let normals = (added.meshlets.as_deref()).map(|meshlets| meshlets.normals.as_slice());
            let geometry = Geometry {
                name: &added.name,
                spirv: &added.spirv,
                shadow: added.shadow.as_deref(),
                normals,
                entry: c"vertex_main",
                mesh: None,
            };
            let made = geometry.pipelines(device, added.blend, added.ids);
            // A model shader's meshlets, through the device's mesh shaders if it has them, with the
            // shader's own fragment shaders.
            let meshlets = (added.meshlets.as_deref()).map(|meshlets| {
                let name = format!("{} meshlets", added.name);
                let geometry = if device.mesh_shaders() {
                    Geometry {
                        name: &name,
                        spirv: &added.spirv,
                        shadow: added.shadow.as_deref(),
                        normals,
                        entry: c"mesh_main",
                        mesh: Some(&meshlets.mesh),
                    }
                } else {
                    Geometry {
                        name: &name,
                        spirv: &meshlets.vertices,
                        shadow: meshlets.vertices_shadow.as_deref(),
                        normals: Some(&meshlets.vertices_normals),
                        entry: c"meshlet_vertex_main",
                        mesh: None,
                    }
                };
                geometry.pipelines(device, added.blend, added.ids)
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
}
