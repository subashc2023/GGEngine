//! The pipelines each shader draws with, made as it's added or recompiled.

use std::ffi::{CStr, CString};
use std::mem;
use std::sync::Arc;
use std::thread::{self, Scope};

use gg_core::{Error, Result};
use gg_gpu::{Blend, Device, Format, GraphicsDesc, Pipeline};
use tracing::warn;

use super::{DepthOnly, GpuShader, MeshPass};
use crate::parallel::{self, Making};
use crate::{Uploads, shader, shadow};

/// A shader's geometry for its pipelines: its module (`name`), its cut-out shadows' module
/// ([`shader::Added::shadow`]), a model shader's normals module ([`shader::Meshlets`]), and its
/// geometry's entry point in each, or in `mesh`, a separate mesh-shader module
/// ([`GraphicsDesc::mesh`]).
struct Geometry<'a> {
    name: &'a str,
    spirv: &'a [u8],
    shadow: Option<&'a [u8]>,
    normals: Option<&'a [u8]>,
    entry: &'a CStr,
    mesh: Option<&'a [u8]>,
}

impl Geometry<'_> {
    /// Its pipeline into the game's view, with `fragment_main`, blending by `blend` and writing IDs
    /// if `ids` ([`shader::Added::ids`]). If it writes depth, also its depth-only pipelines, by its
    /// geometry and `shadow_main` if its shadows are cut out, and its normals by `normal_main`, if
    /// it has their module.
    fn pipelines(
        &self,
        device: &Arc<Device>,
        blend: Blend,
        ids: bool,
    ) -> Result<(Pipeline, Option<DepthOnly>)> {
        let writes_depth = shader::writes_depth(blend);
        let pipeline = || {
            Pipeline::graphics(
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
            )
        };
        // What's opaque casts shadows by its own geometry, and draws depth so for the view's
        // ambient occlusion.
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
        // Normals, into an HDR-format image, tested against the depth-only depth without writing
        // it, so only what's nearest there draws.
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
        // Each on a thread of its own, as a driver compiles a pipeline's shaders as it's made.
        thread::scope(|scope| {
            let (depth_only, normals) = (&depth_only, &normals);
            let pipeline = parallel::make(scope, pipeline);
            let depth_only = writes_depth.then(|| {
                let (shadow, view) = (
                    format!("{} shadow", self.name),
                    format!("{} depth", self.name),
                );
                // One after the other, as they share their shaders, which the second finds compiled.
                let both = move || Ok((depth_only(&shadow, true)?, depth_only(&view, false)?));
                let normals =
                    (self.normals).map(|spirv| parallel::make(scope, move || normals(spirv)));
                (parallel::make(scope, both), normals)
            });
            let pipeline = pipeline.made()?;
            let depth_only = depth_only.map(|(both, normals)| {
                let (shadow, view) = both.made()?;
                let normals = normals.map(Making::made).transpose()?;
                Ok::<_, Error>(DepthOnly {
                    shadow,
                    view,
                    normals,
                })
            });
            Ok((pipeline, depth_only.transpose()?))
        })
    }
}

/// Draws model shader `name`'s meshlets into the visibility buffer, from `meshlets`' modules: by
/// its mesh shader where the device has them, else its vertex shaders.
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

/// A shader's pipelines: its geometry's, with its depth-only pipelines; its meshlets', so too; and
/// its visibility buffer's. A shader without meshlets has neither of the last two.
type Made = (
    (Pipeline, Option<DepthOnly>),
    Option<(Pipeline, Option<DepthOnly>)>,
    Option<Pipeline>,
);

/// Starts making `added`'s pipelines on threads of `scope`'s, each group on one of its own.
fn making<'scope>(
    scope: &'scope Scope<'scope, '_>,
    device: &'scope Arc<Device>,
    added: &'scope shader::Added,
) -> Making<'scope, Made> {
    let normals = (added.meshlets.as_deref()).map(|meshlets| meshlets.normals.as_slice());
    let view = parallel::make(scope, move || {
        let geometry = Geometry {
            name: &added.name,
            spirv: &added.spirv,
            shadow: added.shadow.as_deref(),
            normals,
            entry: c"vertex_main",
            mesh: None,
        };
        geometry.pipelines(device, added.blend, added.ids)
    });
    // A model shader's meshlets, through the device's mesh shaders if it has them, with the
    // shader's own fragment shaders.
    let meshlets = (added.meshlets.as_deref()).map(|meshlets| {
        parallel::make(scope, move || {
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
        })
    });
    // And into the visibility buffer.
    let visibility = (added.meshlets.as_deref()).map(|meshlets| {
        parallel::make(scope, move || {
            visibility_pipeline(device, &added.name, meshlets)
        })
    });
    parallel::make(scope, move || {
        let view = view.made()?;
        let meshlets = meshlets.map(Making::made).transpose()?;
        Ok((view, meshlets, visibility.map(Making::made).transpose()?))
    })
}

impl MeshPass {
    /// Makes the pipelines of `uploads`' shaders, taking them, and drops those of removed shaders.
    /// A recompiled shader's new pipeline replaces the old, which retires until the GPU is done
    /// with it; if the new one can't be made, it warns and keeps the old. A removed shader stays
    /// removed. Then takes the scene's changes as frame `frame`'s, the next to begin.
    ///
    /// # Panics
    ///
    /// - If they come from another `Shaders` than earlier ones.
    pub(crate) fn add_shaders(&mut self, uploads: &mut Uploads, frame: u64) -> Result<()> {
        let _zone = gg_core::zone!();
        // Threads start only for shaders to add: starting a scope allocates, and frames don't.
        if !uploads.shaders.is_empty() {
            let changes = mem::take(&mut uploads.shaders);
            let device = self.device.clone();
            // Every added shader's pipelines on threads of their own, then each change in turn.
            thread::scope(|scope| {
                let making: Vec<_> = (changes.iter())
                    .map(|change| match change {
                        shader::Change::Added(added) => Some(making(scope, &device, added)),
                        shader::Change::Removed(_) => None,
                    })
                    .collect();
                for (change, making) in changes.iter().zip(making) {
                    self.add_shader(change, making)?;
                }
                Ok::<_, Error>(())
            })?;
        }
        self.scene.apply(&mut uploads.scene, frame);
        Ok(())
    }

    /// Takes `change`: drops a removed shader's pipelines, or takes an added one's as `making`
    /// makes them.
    ///
    /// # Panics
    ///
    /// - As [`MeshPass::add_shaders`] does.
    /// - If an added shader has nothing `making`.
    fn add_shader(
        &mut self,
        change: &shader::Change,
        making: Option<Making<'_, Made>>,
    ) -> Result<()> {
        let added = match change {
            shader::Change::Added(added) => added,
            shader::Change::Removed(index) => {
                let shader = self.shaders.get_mut(*index as usize);
                let shader = shader.expect("a renderer draws with the shaders of one `Shaders`");
                // The pipeline retires until the GPU is done with the frames that draw with it.
                *shader = None;
                return Ok(());
            }
        };
        let made = making
            .expect("an added shader's pipelines are being made")
            .made();
        let index = added.index as usize;
        // A recompiled shader replaces one the pass has; any other is its `Shaders`' next.
        let follows = if added.replacement {
            index < self.shaders.len()
        } else {
            index == self.shaders.len()
        };
        assert!(
            follows,
            "a renderer draws with the shaders of one `Shaders`"
        );
        // Draws' sizes are checked against it: its own types, as only the `Shaders` that added it
        // replaces it.
        if let Some(Some(old)) = self.shaders.get(index) {
            assert_eq!(
                (old.vertex_size, old.params_size),
                (added.vertex_size, added.params_size),
                "shader {:?} is replaced by one for other types; a renderer draws with the shaders \
                 of one `Shaders`",
                added.name
            );
        }
        let writes_depth = shader::writes_depth(added.blend);
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
                    // A name with a nul is labeled up to it.
                    label: CString::new(added.name.split('\0').next().unwrap_or_default())
                        .expect("cut at its first nul"),
                    vertex_size: added.vertex_size,
                    params_size: added.params_size,
                    writes_depth,
                    masked: added.meshlets.is_some() && added.shadow.is_some(),
                }));
            }
        }
        Ok(())
    }
}
