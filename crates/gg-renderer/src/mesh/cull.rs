//! Culling a frame's opaque runs on the GPU, for each view that draws them, the game's view and the
//! shadows' cascades, and picking each instance's level of detail there, as `shaders/cull.slang`
//! says; and the indirect draws of what each view keeps, a draw for each run and level, whatever
//! its instances.

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::label;
use gg_core::math::{Mat4, Vec4};
use gg_gpu::{BufferId, ComputePipeline, Device, Frame, Graph, Pass, PassId, RenderPass};

use super::{GpuMesh, Held, LodView, MAX_LEVELS, MeshPass, Push, Uploaded};
use crate::Draws;
use crate::shadow::CASCADES;

pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cull.spv"));

/// How many of a run's instances a workgroup takes: `BLOCK` in `shaders/cull.slang`.
const BLOCK: u64 = 64;

/// The most workgroups a dispatch takes across, the least `maxComputeWorkGroupCount` Vulkan
/// allows: past it, each takes blocks that many apart.
const MOST_GROUPS: u64 = 65_535;

/// The bytes of a `VkDrawIndexedIndirectCommand`.
const COMMAND: u64 = 20;

/// The bytes of an instance's transform and of its ID.
const TRANSFORM: u64 = size_of::<Mat4>() as u64;
const ID: u64 = size_of::<[u32; 2]>() as u64;

/// The most views a frame culls for: the game's view and the shadows' cascades.
pub(crate) const MAX_VIEWS: usize = 1 + CASCADES;

/// The compute pipelines that cull, and what a frame culls, kept between frames, so that one that
/// culls no more than the last allocates nothing.
pub(crate) struct Culling {
    choose: ComputePipeline,
    offsets: ComputePipeline,
    write: ComputePipeline,
    runs: Vec<GpuRun>,
    blocks: Vec<[u32; 2]>,
    levels: Vec<GpuLevel>,
    views: Vec<GpuView>,
    /// For each run of the frame's draws, by its index, its index in `runs`, if it's culled.
    of_run: Vec<Option<u32>>,
    /// The instances of `runs`, and the words of their parameters: what each view keeps room for.
    instances: u32,
    params_words: u32,
}

impl Culling {
    pub(crate) fn new(device: &std::sync::Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        Ok(Self {
            choose: pipeline("cull choose", c"choose_main")?,
            offsets: pipeline("cull offsets", c"offsets_main")?,
            write: pipeline("cull write", c"write_main")?,
            runs: Vec::new(),
            blocks: Vec::new(),
            levels: Vec::new(),
            views: Vec::new(),
            of_run: Vec::new(),
            instances: 0,
            params_words: 0,
        })
    }

    /// The frame's run of the draws' `run`th, if it's culled.
    fn run(&self, run: usize) -> Option<&GpuRun> {
        let culled = (*self.of_run.get(run)?)?;
        Some(&self.runs[culled as usize])
    }

    /// The addresses of the arrays of the instances `views` views keep, in the graph's buffer at
    /// `buffer`: their transforms, IDs and parameters, each array's one view's after another's.
    fn kept(&self, buffer: u64, views: u64) -> [u64; 3] {
        let ids = buffer + views * u64::from(self.instances) * TRANSFORM;
        [buffer, ids, ids + views * u64::from(self.instances) * ID]
    }
}

/// The most a frame's culling takes: of the runs it may cull, how many there are, their
/// instances, the blocks of [`BLOCK`] they're taken in, a run's apart from the rest's, and the
/// words of their parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bounds {
    runs: u64,
    instances: u64,
    blocks: u64,
    params_words: u64,
}

impl Bounds {
    /// Each view's room for the instances it keeps: their transforms, IDs and parameters.
    fn kept_bytes(&self) -> u64 {
        self.instances * (TRANSFORM + ID) + self.params_words * 4
    }

    /// Each view's room for its commands: a run's levels' each, at most [`MAX_LEVELS`].
    fn commands_bytes(&self) -> u64 {
        self.runs * MAX_LEVELS as u64 * COMMAND
    }

    /// Each view's room for its instances' places, then its blocks' counts of each level.
    fn scratch_bytes(&self) -> u64 {
        self.instances * 4 + self.blocks * MAX_LEVELS as u64 * 4
    }
}

/// What a frame's culling takes of its graph: the buffers it writes, of a view's room each for
/// as many views as it culls for, what it reads back, and its passes, which go before those that
/// draw what it keeps.
pub(crate) struct CullPlan {
    views: usize,
    bounds: Bounds,
    /// The instances each view keeps, as [`Culling::kept`] lays them out.
    instances: BufferId,
    /// Each view's commands.
    commands: BufferId,
    /// Each view's instances' places and its blocks' counts.
    scratch: BufferId,
    /// What the first view drew, as `shaders/cull.slang` counts it.
    counts: BufferId,
    choose: PassId,
    offsets: PassId,
    write: PassId,
    read: PassId,
}

impl CullPlan {
    /// Declares in `graph` the culling of runs within `bounds`, for `views` views, which go in the
    /// order of the [`CullView`]s [`MeshPass::cull`] is given: the passes that draw what it keeps
    /// come after it, and take it as [`CullPlan::drawn`] has them.
    pub(crate) fn new(graph: &mut Graph, bounds: Bounds, views: usize) -> Self {
        let count = views as u64;
        let instances = graph.buffer("culled instances", count * bounds.kept_bytes());
        let commands = graph.buffer("culled draws", count * bounds.commands_bytes());
        let scratch = graph.buffer("culling", count * bounds.scratch_bytes());
        let counts = graph.buffer("culled counts", 16);
        let choose = (graph.compute(label!("cull choose")))
            .write_buffer(scratch)
            .write_buffer(counts)
            .add();
        let offsets = (graph.compute(label!("cull offsets")))
            .write_buffer(scratch)
            .write_buffer(commands)
            .write_buffer(counts)
            .add();
        let write = (graph.compute(label!("cull write")))
            .read_buffer(scratch)
            .write_buffer(instances)
            .add();
        let read = graph.read_back(label!("read culled counts"), counts);
        Self {
            views,
            bounds,
            instances,
            commands,
            scratch,
            counts,
            choose,
            offsets,
            write,
            read,
        }
    }

    /// `pass`, taking the instances and commands of what the culling keeps.
    pub(crate) fn drawn<'g>(&self, pass: RenderPass<'g>) -> RenderPass<'g> {
        pass.read_buffer(self.instances).indirect(self.commands)
    }
}

/// A view a frame culls for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CullView {
    /// From the world to its clip space.
    pub(crate) clip_from_world: Mat4,
    /// Its size in pixels, across then down, which levels of detail are picked by.
    pub(crate) size: [u32; 2],
    /// Whether its depth is clamped at its near side, as a shadow cascade's is, so that what's
    /// nearer than that side draws there, and isn't culled.
    pub(crate) clamped: bool,
}

/// What a frame drew in the game's view, as `shaders/cull.slang` counts it: draws, instances, and
/// the low and the high word of the triangles.
pub(crate) fn counts(read: &[u8]) -> Option<crate::DrawCounts> {
    let (words, _) = read.as_chunks::<4>();
    let [draws, instances, low, high] = *words.first_chunk::<4>()?;
    let word = |bytes: [u8; 4]| u64::from(u32::from_ne_bytes(bytes));
    Some(crate::DrawCounts {
        draws: word(draws) as usize,
        instances: word(instances) as usize,
        triangles: (word(high) << 32 | word(low)) as usize,
    })
}

/// A run, as `Run` in `shaders/cull.slang` has it.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuRun {
    first: u32,
    count: u32,
    params: u32,
    params_words: u32,
    sphere: Vec4,
    levels: u32,
    level_count: u32,
    output: u32,
    params_output: u32,
    commands: u32,
    blocks: u32,
    block_count: u32,
}

/// A level of detail, as `Level` in `shaders/cull.slang` has it.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuLevel {
    start: u32,
    count: u32,
    error: f32,
}

/// A view, as `View` in `shaders/cull.slang` has it.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuView {
    planes: [Vec4; 6],
    w: Vec4,
    w_slope: f32,
    pixels_per_unit: f32,
}

/// What the culling pushes, as `Push` in `shaders/cull.slang` has it: every address it takes, as
/// it says why.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct CullPush {
    runs: u64,
    blocks: u64,
    levels: u64,
    views: u64,
    transforms: u64,
    ids: u64,
    params: u64,
    kept_transforms: u64,
    kept_ids: u64,
    kept_params: u64,
    commands: u64,
    scratch: u64,
    counts: u64,
    run_count: u32,
    block_count: u32,
    groups: u32,
    instances: u32,
    params_words: u32,
    /// Making the struct a whole number of its alignment, as bytemuck has it.
    unused: u32,
}

/// The planes of what `clip_from_world` takes into Vulkan's clip space, each inside where a point's
/// dot product with its XYZ, plus its W, is at least 0: its sides, then its near side, where depth
/// is 1, or, `clamped`, no plane there, then its far side, where depth is 0 (Gribb and Hartmann,
/// "Fast Extraction of Viewing Frustum Planes from the World-View-Projection Matrix"). Seeing to
/// infinity, the far side's plane holds everything.
fn planes(clip_from_world: &Mat4, clamped: bool) -> [Vec4; 6] {
    let [x, y, z, w] = [0, 1, 2, 3].map(|row| clip_from_world.row(row));
    let near = if clamped { Vec4::W } else { w - z };
    [w + x, w - x, w + y, w - y, near, z]
}

impl MeshPass {
    /// The most a frame's culling of `draws` takes, of their runs whose shaders are opaque, by the
    /// shaders the pass has, whatever their meshes; `None` if no run's is.
    pub(crate) fn cull_bounds(&self, draws: &Draws) -> Option<Bounds> {
        let mut bounds = Bounds::default();
        for run in &draws.runs {
            let Some(Some(shader)) = self.shaders.get(run.shader as usize) else {
                continue;
            };
            if shader.writes_depth {
                let count = run.instances.len() as u64;
                bounds.runs += 1;
                bounds.instances += count;
                bounds.blocks += count.div_ceil(BLOCK);
                bounds.params_words += count * (shader.params_size / 4) as u64;
            }
        }
        (bounds.runs > 0).then_some(bounds)
    }

    /// Culls the runs of `draws` whose meshes are drawable and whose shaders are opaque, as `plan`
    /// declared in the frame's graph, for each of `views`, from their instances as `uploaded` has
    /// them, and records its passes, the next of the frame's, and what reads back what the first
    /// view drew. Returns whether it culled any run: the frame's read back counts nothing if it
    /// didn't, as no meshes it may cull are drawable yet.
    ///
    /// # Panics
    ///
    /// If `views` aren't as many as `plan` was declared for, or `draws` aren't what bounded it.
    pub(crate) fn cull(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &CullPlan,
        draws: &Draws,
        uploaded: Uploaded,
        views: &[CullView],
    ) -> Result<bool> {
        let _zone = gg_core::zone!();
        assert_eq!(
            views.len(),
            plan.views,
            "a culling's views are those it was planned for"
        );
        let culling = &mut self.culling;
        culling.runs.clear();
        culling.blocks.clear();
        culling.levels.clear();
        culling.views.clear();
        culling.of_run.clear();
        let (mut instances, mut params_words, mut commands) = (0, 0, 0);
        for (i, run) in draws.runs.iter().enumerate() {
            culling.of_run.push(None);
            let mesh = self.meshes.get(run.mesh as usize).and_then(Held::ready);
            let shader = self
                .shaders
                .get(run.shader as usize)
                .and_then(Option::as_ref);
            let (Some(mesh), Some(shader)) = (mesh, shader) else {
                continue;
            };
            if !shader.writes_depth {
                continue;
            }
            let GpuMesh { lods, .. } = mesh;
            let count = u32::try_from(run.instances.len())
                .expect("a run's instances fit the 32-bit count Vulkan takes");
            let words = (shader.params_size / 4) as u32;
            let (first_block, block_count) = (culling.blocks.len(), count.div_ceil(BLOCK as u32));
            let run_index = culling.runs.len() as u32;
            (culling.blocks)
                .extend((0..block_count).map(|block| [run_index, block * BLOCK as u32]));
            let first_level = culling.levels.len();
            (culling.levels).extend(lods.levels.iter().map(|lod| GpuLevel {
                start: lod.start,
                count: lod.count,
                error: lod.error,
            }));
            culling.runs.push(GpuRun {
                first: run.instances.start as u32,
                count,
                params: (run.params.start / 4) as u32,
                params_words: words,
                sphere: lods.center.extend(lods.radius),
                levels: first_level as u32,
                level_count: lods.levels.len() as u32,
                output: instances,
                params_output: params_words,
                commands,
                blocks: first_block as u32,
                block_count,
            });
            culling.of_run[i] = Some(run_index);
            instances += count;
            params_words += count * words;
            commands += lods.levels.len() as u32;
        }
        // What the GPU writes stays within what the graph's buffers were made for.
        let bounds = plan.bounds;
        assert!(
            culling.runs.len() as u64 <= bounds.runs
                && u64::from(instances) <= bounds.instances
                && culling.blocks.len() as u64 <= bounds.blocks
                && u64::from(params_words) <= bounds.params_words,
            "a culling's draws are those it was planned for"
        );
        (culling.instances, culling.params_words) = (instances, params_words);
        let (run_count, block_count) = (culling.runs.len() as u64, culling.blocks.len() as u64);
        if run_count == 0 {
            // Its passes are the frame's all the same.
            drop(frame.compute(plan.choose));
            drop(frame.compute(plan.offsets));
            drop(frame.compute(plan.write));
            frame.read_back(plan.read)?;
            return Ok(false);
        }
        culling.views.extend(views.iter().map(|view| {
            let lod = LodView::new(&view.clip_from_world, view.size);
            GpuView {
                planes: planes(&view.clip_from_world, view.clamped),
                w: lod.w,
                w_slope: lod.w_slope,
                pixels_per_unit: lod.pixels_per_unit,
            }
        }));
        let [kept_transforms, kept_ids, kept_params] =
            culling.kept(frame.address(plan.instances), plan.views as u64);
        let (commands, scratch, counts) = (
            frame.address(plan.commands),
            frame.address(plan.scratch),
            frame.address(plan.counts),
        );
        let mut address = |bytes: &[u8]| -> Result<u64> { Ok(frame.upload(bytes)?.address()) };
        let groups = block_count.min(MOST_GROUPS) as u32;
        let push = CullPush {
            runs: address(bytemuck::cast_slice(&culling.runs))?,
            blocks: address(bytemuck::cast_slice(&culling.blocks))?,
            levels: address(bytemuck::cast_slice(&culling.levels))?,
            views: address(bytemuck::cast_slice(&culling.views))?,
            transforms: uploaded.transforms,
            ids: uploaded.ids,
            params: uploaded.params,
            kept_transforms,
            kept_ids,
            kept_params,
            commands,
            scratch,
            counts,
            run_count: run_count as u32,
            block_count: block_count as u32,
            groups,
            instances,
            params_words,
            unused: 0,
        };
        let views = views.len() as u32;
        let passes = [
            (plan.choose, &culling.choose, groups),
            (
                plan.offsets,
                &culling.offsets,
                run_count.div_ceil(BLOCK) as u32,
            ),
            (plan.write, &culling.write, groups),
        ];
        for (pass, pipeline, across) in passes {
            let mut pass = frame.compute(pass);
            pass.set_pipeline(pipeline);
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch([across, views, 1]);
        }
        frame.read_back(plan.read)?;
        Ok(true)
    }

    /// Draws the instances that view `view` of `plan` kept of the draws' `run`th, if it's culled,
    /// with `push`'s mesh, a draw for each of its mesh's levels, as the commands written for them
    /// say: `None` if it isn't culled.
    pub(super) fn draw_culled(
        &self,
        pass: &mut Pass<'_, '_>,
        mesh: &GpuMesh,
        push: Push,
        (plan, view): (&CullPlan, usize),
        run: usize,
    ) -> Option<()> {
        let culling = &self.culling;
        let run = culling.run(run)?;
        let [transforms, ids, params] =
            culling.kept(pass.address(plan.instances), plan.views as u64);
        let view = view as u64;
        let first = view * u64::from(culling.instances) + u64::from(run.output);
        let params = if run.params_words == 0 {
            0
        } else {
            params + (view * u64::from(culling.params_words) + u64::from(run.params_output)) * 4
        };
        let push = Push {
            transforms: transforms + first * TRANSFORM,
            ids: ids + first * ID,
            params,
            ..push
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        let runs = culling.runs.len() as u64;
        let commands = (view * runs * MAX_LEVELS as u64 + u64::from(run.commands)) * COMMAND;
        for level in 0..u64::from(run.level_count) {
            pass.draw_indexed_indirect(&mesh.indices, plan.commands, commands + level * COMMAND);
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Projection, Viewpoint};
    use gg_core::math::{Quat, Vec3};

    /// A point inside each plane is in what the view sees.
    fn inside(planes: &[Vec4; 6], point: Vec3) -> bool {
        planes
            .iter()
            .all(|plane| plane.truncate().dot(point) + plane.w >= 0.0)
    }

    /// A perspective's planes hold what's ahead of it within its angle, however far, and nothing
    /// behind it or nearer than its near plane; a clamped view's hold what's nearer too.
    #[test]
    fn a_views_planes_hold_what_it_sees() {
        let viewpoint = Viewpoint {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            projection: Projection::Perspective {
                fov: std::f32::consts::FRAC_PI_2,
                near: 0.1,
            },
        };
        let clip = viewpoint.view_projection(1.0);
        let seen = planes(&clip, false);
        assert!(inside(&seen, Vec3::new(0.0, 0.0, -1.0)));
        assert!(
            inside(&seen, Vec3::new(0.0, 0.0, -1.0e6)),
            "seen to infinity"
        );
        assert!(
            inside(&seen, Vec3::new(0.9, -0.9, -1.0)),
            "within its angle"
        );
        assert!(!inside(&seen, Vec3::new(1.1, 0.0, -1.0)), "past its side");
        assert!(!inside(&seen, Vec3::new(0.0, 0.0, 1.0)), "behind");
        assert!(
            !inside(&seen, Vec3::new(0.0, 0.0, -0.05)),
            "nearer than its near plane"
        );
        let clamped = planes(&clip, true);
        assert_eq!(clamped[4], Vec4::W, "no plane at its near side");
    }

    /// The counts read back come apart into the draws, the instances and the 64-bit triangles.
    #[test]
    fn counts_read_back_hold_triangles_past_32_bits() {
        let words = [3u32, 40, 5, 1].map(u32::to_ne_bytes).concat();
        let counts = counts(&words).expect("four words");
        assert_eq!(
            (counts.draws, counts.instances, counts.triangles),
            (3, 40, (1 << 32) + 5)
        );
        assert_eq!(super::counts(&words[..12]), None, "too few");
    }
}
