//! Drawing a pass's runs: what each draw pushes and reads, and its level of detail.

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
#[cfg(doc)]
use gg_gpu::Format;
use gg_gpu::{Frame, Pass};

use super::{CullPlan, FIRST, GpuMesh, Lods, MeshPass};
use crate::draws::Run;
use crate::particles::ParticleDraws;
use crate::{DrawCounts, Draws};

/// The addresses of a frame's draws' transforms, IDs and parameters, as [`MeshPass::upload_draws`]
/// copied them; 0 for none.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Uploaded {
    pub(super) transforms: u64,
    pub(super) ids: u64,
    pub(super) params: u64,
}

impl MeshPass {
    /// Copies `draws`' transforms, IDs and parameters into the frame, each array once for every
    /// pass and the culling.
    pub(crate) fn upload_draws(frame: &mut Frame<'_>, draws: &Draws) -> Result<Uploaded> {
        // An upload per array, so each starts 16-byte aligned.
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

    /// Draws `runs` of `draws` and the scene into the pass's target (`view` pixels), `frame` being
    /// a [`FrameData`]'s address. Culled runs draw first, then the rest in the draws' order, the
    /// scene's at its place. A culled run draws what its view kept, a draw per level; any other is
    /// one instanced draw, or one per stretch of instances at one level. The scene's runs draw only
    /// culled. Runs of undrawable or removed meshes or shaders draw nothing. Particles draw in
    /// their place where the pass shades. Returns what it drew of the unculled runs.
    ///
    /// # Panics
    ///
    /// - If a mesh or shader isn't the pass's.
    /// - If vertices or parameters aren't the size the shader reads.
    /// - If a run has more instances than Vulkan's 32-bit count.
    /// - If the target isn't what `way` draws into: a [`Format::HDR`] image or a visibility buffer,
    ///   with IDs and depth, or depth alone.
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
        // State set so far, which binding a pipeline leaves alone. The scissor is the view's own
        // until a run's clip.
        let (mut bound, mut faces, mut clipped) = (None, None, None);
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
            // Its mesh, shader and faces, the draws' run if it's theirs, and its culled-run index
            // if culled.
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
            // Each in its turn, and the scene's runs only culled.
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
            // Its clip as a scissor; nothing for a clip the view doesn't show.
            let clip = run.and_then(|run| run.clip);
            if clip != clipped {
                let (origin, size) =
                    clip.map_or(([0, 0], view), |clip| scissor(clip, view_projection, view));
                if size.contains(&0) {
                    continue;
                }
                pass.set_scissor(origin, size);
                clipped = clip;
            }
            // An emitter's particles, never culled, on the quad, whose indices they take.
            if let Some(index) = run.and_then(|run| run.particles) {
                if let (Way::Shaded { .. }, Some(particles)) = (way, particles) {
                    pass.begin_label(c"particles");
                    drawn += particles.draw(pass, &mesh.indices, index, view_projection, frame);
                    pass.end_label();
                    // Particles bound their own pipeline and faces in place of the runs'.
                    (bound, faces) = (None, None);
                }
                continue;
            }
            // A run culled by meshlets draws by its shader's meshlet geometry.
            let by_meshlets = culled_run.is_some_and(|run| self.culling.by_meshlets(run));
            let (geometry, depth_only) = match &shader.meshlets {
                Some((geometry, depth_only)) if by_meshlets => (geometry, depth_only),
                _ => (&shader.pipeline, &shader.depth_only),
            };
            // A model shader's such run draws into the view's visibility buffer, if it has one.
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
                (Way::Normals { beside_visibility }, visible) => {
                    let Some(normals) = depth_only.as_ref().and_then(|d| d.normals.as_ref()) else {
                        continue;
                    };
                    if beside_visibility && visible.is_some() {
                        continue;
                    }
                    normals
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
                // Each draw sets its instances' addresses.
                transforms: 0,
                ids: 0,
                params: 0,
                view: view.map(|side| side as f32),
                frame,
                // A meshlet draw sets them, and a culled draw of the game's view its last-frame
                // transforms.
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
        if clipped.is_some() {
            pass.set_scissor([0, 0], view);
        }
        Ok(drawn)
    }
}

/// The pixels of a `view`-sized view (origin and size, from its top left) inside `clip`
/// ([`Draws::clip`]) as `view_projection` shows it: the box round its corners, at the nearest pixel
/// edges, within the view.
pub(super) fn scissor(
    clip: [Vec2; 2],
    view_projection: Mat4,
    view: [u32; 2],
) -> ([u32; 2], [u32; 2]) {
    let [least, most] = clip;
    let size = Vec2::new(view[0] as f32, view[1] as f32);
    let corners = [
        least,
        Vec2::new(most.x, least.y),
        most,
        Vec2::new(least.x, most.y),
    ];
    let (mut low, mut high) = (Vec2::INFINITY, Vec2::NEG_INFINITY);
    for corner in corners {
        let at = view_projection * corner.extend(0.0).extend(1.0);
        // Vulkan's clip space is -1 at the view's left and top, and 1 at its right and bottom.
        let pixel = (at.truncate().truncate() / at.w + 1.0) / 2.0 * size;
        (low, high) = (low.min(pixel), high.max(pixel));
    }
    let [low, high] = [low, high].map(|edge| edge.round().clamp(Vec2::ZERO, size));
    let extent = (high - low).max(Vec2::ZERO);
    // A NaN clip, as one past the camera, casts to none.
    (
        [low.x as u32, low.y as u32],
        [extent.x as u32, extent.y as u32],
    )
}

/// How [`MeshPass::draw`] draws a pass's runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Way {
    /// Into HDR colors: every run, or beside a visibility buffer, all but those it shades.
    Shaded { beside_visibility: bool },
    /// Into a shadow map, by the shadow pipelines of runs that write depth.
    Shadows,
    /// Depth alone, for the view's ambient occlusion, by the depth pipelines of runs that write
    /// depth; beside a visibility buffer, not those it holds.
    Depth { beside_visibility: bool },
    /// Normals for reflections, where nearest in [`Way::Depth`]'s depth, by the opaque and masked
    /// model shaders' normals pipelines; beside a visibility buffer, not those it covers.
    /// Reflections find the rest's from depth.
    Normals { beside_visibility: bool },
    /// Into a visibility buffer: culled runs drawn by meshlets, of the opaque and masked model
    /// shaders.
    Visibility,
}

/// Which of a frame's runs [`MeshPass::draw`] draws.
#[derive(Clone, Copy)]
pub(crate) enum Runs<'p> {
    /// What the culling's view in that slot kept, alone: the game's view's first pass, or a
    /// cascade's.
    Culled(&'p CullPlan, usize),
    /// What the culling's view in that slot kept, then the rest: the game's view's second pass.
    CulledThenRest(&'p CullPlan, usize),
    /// The unculled runs alone, what blends: the game's view's pass after fog, which fogs the
    /// earlier passes.
    Rest(&'p CullPlan),
    /// Every run, at CPU-picked levels: the canvas, or a view whose frame culls none.
    All,
}

/// A run a pass draws: the draws' run of that index, or the scene's.
#[derive(Clone, Copy)]
enum Drawn {
    Frame(usize),
    Scene(usize),
}

/// Pixels a level's error may span on screen for a draw to use it: one, the least a view resolves.
const LOD_ERROR_PIXELS: f32 = 1.0;

/// What picking levels of detail needs of a pass's view, from `C`, its world-to-clip matrix: a
/// projection times a rigid view. In both projections clip W is the distance ahead of the camera:
/// perspective's fourth row is (0, 0, -1, 0), orthographic's (0, 0, 0, 1). Clip X's and Y's rows
/// are the projection's scale times a rotation row, so their XYZ's length is the scale: clip units
/// (half the view) per world unit where W is 1.
pub(super) struct LodView {
    /// `C`'s fourth row: a point's clip W is its dot product with the point, W 1.
    pub(super) w: Vec4,
    /// The length of `w`'s XYZ: W's change per world unit of distance, 1 under perspective and 0
    /// under orthographic.
    pub(super) w_slope: f32,
    /// Pixels a world unit spans where W is 1, the larger of across and down.
    pub(super) pixels_per_unit: f32,
}

impl LodView {
    pub(super) fn new(view_projection: &Mat4, view: [u32; 2]) -> Self {
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

    /// The coarsest of `lods`' levels whose error spans at most [`LOD_ERROR_PIXELS`] where the mesh
    /// is nearest; else level 0, as when the mesh reaches the camera's plane. The error spans
    /// `error * scale * pixels_per_unit / near` pixels, `scale` being `transform`'s longest axis
    /// and `near` the W of the sphere's nearest point (W less the radius).
    pub(super) fn level(&self, lods: &Lods, transform: &Mat4) -> usize {
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
        // Errors never decrease by level, so the last to pass is the coarsest. A NaN passes none.
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
    /// Bytes of one instance's parameters.
    params_size: usize,
}

/// Draws `instances` of `mesh` directly with `push`, setting the instances per draw: one instanced
/// draw, or one per stretch of consecutive instances at the level `view` picks, in order. Returns
/// what it drew.
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

/// What the mesh pass pushes before each draw: `Push` in `shaders/mesh.slang`. Games' shaders read
/// its start ([`Shaders::add`](crate::Shaders::add)). The matrix leads, so offsets match under
/// every Slang push-constant layout. IDs precede parameters, so a shader taking none ends its
/// struct after its last member. Later fields were appended, moving no offset games declare;
/// shaders without parameters can't read them.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Push {
    /// From the world to clip space, column-major.
    pub(crate) clip_from_world: Mat4,
    /// Address of the mesh's vertices.
    pub(crate) vertices: u64,
    /// Address of the instances' transforms.
    pub(crate) transforms: u64,
    /// Address of the instances' IDs, each the low and high halves of a 64-bit number.
    pub(crate) ids: u64,
    /// Address of the instances' parameters; 0 for a shader that takes none.
    pub(crate) params: u64,
    /// The pass's view size in pixels, across then down.
    pub(crate) view: [f32; 2],
    /// The address of a [`FrameData`].
    pub(crate) frame: u64,
    /// For a culled run's meshlet draw, the culling's draw address, its kept meshlets following;
    /// else 0.
    pub(crate) meshlets: u64,
    /// For a visibility-buffer draw: 1 past twice its culled run's index, plus 1 in the second
    /// pass, as the buffer holds it; else 0, none.
    pub(crate) draw: u32,
    /// Transforms past each instance's to its last-frame one, by which the view moves draws. For
    /// the game's view's culled draws, where the culling kept them; else 0, reading their own (a
    /// layer's draws are made each frame), so they move with the camera alone.
    pub(crate) previous: u32,
}

/// What every draw's shader may read of the frame, through [`Push::frame`]: `Frame` in
/// `shaders/mesh.slang`, field for field. Fields are added at its end, so a shader reading those
/// before them keeps to the contract.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
pub(crate) struct FrameData {
    /// The view's position in the world.
    pub(crate) camera: Vec3,
    /// In seconds ([`RenderData::time`](crate::RenderData::time)).
    pub(crate) time: f32,
    /// What lit shading multiplies light by
    /// ([`RenderData::exposure`](crate::RenderData::exposure)).
    pub(crate) exposure: f32,
    /// How many lights the frame has ([`RenderData::lights`](crate::RenderData::lights)).
    pub(crate) light_count: u32,
    /// Address of its lights ([`GpuLight`](crate::light::GpuLight)), in
    /// [`Lighting::lights`](crate::cluster::Lighting::lights)' order, or 0. In the game's view with
    /// lights binned, the copies the clusters follow
    /// ([`Lighting::bin`](crate::cluster::Lighting::bin)).
    pub(crate) lights: u64,
    /// The address of the materials table, by index, or 0 before any is added.
    pub(crate) materials: u64,
    /// Where lit shading sees from: the camera and 1, or under orthographic the way back and 0. The
    /// way to the eye from `p` is `eye.xyz - p * eye.w`.
    pub(crate) eye: Vec4,
    /// The sky and its prefiltered light ([`SkyData`](crate::sky::SkyData)).
    pub(crate) sky: crate::sky::SkyData,
    /// The shadow-casting light's shadows, if any ([`ShadowData`](crate::shadow::ShadowData)).
    pub(crate) shadows: crate::shadow::ShadowData,
    /// The clusters lights are binned into ([`ClusterData`](crate::cluster::ClusterData)).
    pub(crate) clusters: crate::cluster::ClusterData,
    /// The bindless slot of the view's ambient occlusion
    /// ([`AmbientOcclusion`](crate::AmbientOcclusion)); 0 for none, read as 1, as in all passes but
    /// the view's shading.
    pub(crate) ambient_occlusion: u32,
    /// What lit shading returns in place of light, by [`Channel::index`](crate::Channel::index); 0
    /// for light.
    pub(crate) show: u32,
    /// The view's height fog ([`FogData`](crate::fog::FogData)).
    pub(crate) fog: crate::fog::FogData,
    /// The atmosphere's haze ([`AerialData`](crate::atmosphere::AerialData)).
    pub(crate) aerial: crate::atmosphere::AerialData,
    /// The game's view's reflections ([`ReflectionData`](crate::reflections::ReflectionData)); none
    /// but in the passes shading the view.
    pub(crate) reflections: crate::reflections::ReflectionData,
}
