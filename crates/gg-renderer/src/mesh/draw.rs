//! Drawing a pass's runs: what each draw pushes and reads, and the level of detail it picks.

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

/// Where a frame's draws' instances are, as [`MeshPass::upload_draws`] copied them into the frame:
/// the addresses of their transforms, their IDs and their parameters, 0 for none.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Uploaded {
    pub(super) transforms: u64,
    pub(super) ids: u64,
    pub(super) params: u64,
}

impl MeshPass {
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
        // What's set, which a pipeline's binding leaves as it is: the scissor the view's own until
        // a run's clip.
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
            // Its clip, as a scissor, and nothing of a run whose clip the view doesn't show.
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
        if clipped.is_some() {
            pass.set_scissor([0, 0], view);
        }
        Ok(drawn)
    }
}

/// The pixels of a view `view` across and down, from its top left, that `clip` shows within, a
/// rectangle in the XY plane of the space `view_projection` takes to clip space, its least corner
/// then its greatest, as [`Draws::clip`] says: the box round where its corners show, each edge at
/// the nearest pixel's, so that a pixel draws whose middle is inside, kept within the view; its
/// origin and its size.
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
    // A clip of no number, as one past the camera, is cast to none.
    (
        [low.x as u32, low.y as u32],
        [extent.x as u32, extent.y as u32],
    )
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
    /// Their normals, for the view's reflections, as their shading turns them, where they're nearest
    /// in the depth [`Way::Depth`] drew, which they're tested against, by the normals' pipelines of
    /// the opaque and masked model shaders: every such run, or, beside a visibility buffer, all but
    /// those drawn into it, whose normals are found from it; and nothing of the rest, whose normals
    /// the reflections find from the depth.
    Normals { beside_visibility: bool },
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
pub(super) struct LodView {
    /// `C`'s fourth row: a point's clip W is its dot product with the point, with a W of 1.
    pub(super) w: Vec4,
    /// The length of `w`'s XYZ: how much W changes with a world unit's distance, 1 under
    /// perspective and 0 under orthographic.
    pub(super) w_slope: f32,
    /// The pixels a world unit spans where W is 1, the larger of across and down.
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

    /// The coarsest of `lods`' levels whose error, as `transform` places the mesh in the world,
    /// spans at most [`LOD_ERROR_PIXELS`] on screen where the mesh is nearest, or level 0 if none
    /// does, or the mesh reaches the plane of the camera or crosses it.
    ///
    /// `transform` scales the mesh's units by the longest of its axes, which is how it scales the
    /// error and the sphere. The error spans `error * scale * pixels_per_unit / near` pixels, where
    /// `near` is the W of the sphere's nearest point, W less the radius, at most.
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
    /// What the view shows in place of its light, as [`Channel::index`](crate::Channel::index)
    /// numbers it, which lit shading returns in place of its light: 0 for its light.
    pub(crate) show: u32,
    /// The height fog the view is in, as [`FogData`](crate::fog::FogData) has it.
    pub(crate) fog: crate::fog::FogData,
    /// The atmosphere's haze, as [`AerialData`](crate::atmosphere::AerialData) has it.
    pub(crate) aerial: crate::atmosphere::AerialData,
    /// The game's view's reflections, as
    /// [`ReflectionData`](crate::reflections::ReflectionData) has them: none but in the passes that
    /// shade the view.
    pub(crate) reflections: crate::reflections::ReflectionData,
}
