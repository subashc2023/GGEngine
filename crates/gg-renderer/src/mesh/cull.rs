//! GPU culling of a frame's opaque runs, the draws' and the scene's, per view: the game's view's
//! two passes and the shadow cascades. The second pass also tests the depth pyramid of the first's
//! depth.
//!
//! `shaders/gather.slang` gathers the instances, `shaders/cull.slang` culls them and picks their
//! levels of detail, and `shaders/meshlets.slang` culls kept instances' meshlets. Each view draws
//! what it keeps indirectly: a draw per run and level, or one of a run's kept meshlets.

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::label;
use gg_core::math::{Mat3, Mat4, Vec3, Vec4};
use gg_gpu::{
    BufferId, Compute, ComputePass, ComputePipeline, Device, Faces, Frame, Graph, ImageId, Pass,
    PassId, RenderPass,
};

use super::draw::{LodView, Uploaded};
use super::scene::SceneRun;
use super::{GpuMesh, GpuShader, Held, MAX_LEVELS, MeshPass, PlacedMeshlets, Push, Way};
use crate::shadow::{CASCADES, OUTLINE};
use crate::{Draws, MeshDraw, halves};

pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cull.spv"));
pub(crate) static GATHER_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gather.spv"));
pub(crate) static PYRAMID_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pyramid.spv"));
pub(crate) static MESHLETS_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/meshlets.spv"));

/// A run drawn by indices has this first meshlet: `NO_MESHLETS` in `shaders/culling.slang`.
const NO_MESHLETS: u32 = u32::MAX;

/// Workgroups across a mesh-shader meshlet draw, rows as needed: `ACROSS` in
/// `shaders/meshlets.slang` and `shaders/model.slang`.
const MESHLET_ACROSS: u64 = 32_768;

/// The most meshlets a run's draw takes per view: `VK_EXT_mesh_shader`'s least
/// `maxMeshWorkGroupTotalCount`, `MESHLET_ACROSS` by 128 rows. Kept meshlets past it are dropped.
const MOST_MESHLETS: u64 = 1 << 22;

// Within `VK_EXT_mesh_shader`'s least `maxMeshWorkGroupCount` each way.
const _: () = assert!(MESHLET_ACROSS <= 65_535 && MOST_MESHLETS / MESHLET_ACROSS <= 65_535);

/// Words of a run's meshlet draw before its kept meshlets (two words each), per `offsets_main` in
/// `shaders/cull.slang`: a `VkDrawIndirectCommand`, a `VkDrawMeshTasksIndirectCommandEXT` and the
/// count kept.
const MESHLET_DRAW: u64 = 8;

/// A run's meshlet room per view: every meshlet of its largest level, per instance, up to
/// [`MOST_MESHLETS`].
fn meshlet_room(placed: &PlacedMeshlets, count: u64) -> u64 {
    (count * u64::from(placed.most)).min(MOST_MESHLETS)
}

/// A run's meshlet words per view: its draw and its [`meshlet_room`].
fn meshlet_words(placed: &PlacedMeshlets, count: u64) -> u64 {
    MESHLET_DRAW + 2 * meshlet_room(placed, count)
}

/// The most meshlet words a frame plans per view; runs past it draw by their levels' indices,
/// without meshlet culling or a visibility buffer. It fits one run's largest draw: 32 MiB a view,
/// 64 MiB for the game's view's two passes, 192 MiB with cascades drawn through mesh shaders.
///
/// Room is planned per meshlet of every instance, so without a ceiling a few runs of many instances
/// (foliage) outgrow GPU memory, and a frame that can't make its buffers ends the renderer. Room
/// for a second such run would cost 8 bytes per meshlet per view, and the graph's power-of-two
/// rounding takes six views from 256 MiB to 512 MiB.
const MOST_MESHLET_WORDS: u64 = MESHLET_DRAW + 2 * MOST_MESHLETS;

/// `source_params` of a copy from the scene: `SCENE` in `shaders/gather.slang`.
const SCENE: u32 = u32::MAX;

/// Instances per workgroup: `BLOCK` in `shaders/cull.slang`.
const BLOCK: u64 = 64;

/// The most workgroups across a dispatch, Vulkan's least `maxComputeWorkGroupCount`; past it, each
/// takes blocks that many apart. `MOST_GROUPS` in `shaders/cull.slang`.
const MOST_GROUPS: u64 = 65_535;

/// Bytes of a `VkDrawIndexedIndirectCommand`.
const COMMAND: u64 = 20;

/// Bytes of an instance's transform and ID.
const TRANSFORM: u64 = size_of::<Mat4>() as u64;
const ID: u64 = size_of::<[u32; 2]>() as u64;

/// Pyramid workgroups' side, a texel per thread: `numthreads` in `shaders/pyramid.slang`.
const PYRAMID_GROUP: u32 = 8;

/// One per halving of a side of up to 2^32 pixels.
const MOST_PYRAMID_LEVELS: usize = 32;

/// The game's view's first pass's slot: `FIRST` in `shaders/cull.slang`. The cascades follow if
/// drawn, each at 1 past its index, then the second pass ([`CullPlan::second`]).
pub(crate) const FIRST: usize = 0;

/// The game's view's two passes and the shadow cascades.
pub(crate) const MAX_VIEWS: usize = 2 + CASCADES;

/// What a view keeps inside its planes: `KEEP_ALL`, `KEEP_SHOWN` and `KEEP_UNHIDDEN` in
/// `shaders/cull.slang`.
#[derive(Clone, Copy)]
#[repr(u32)]
enum Keep {
    /// A cascade's: all.
    All = 0,
    /// The first pass's: what the last frame found unhidden.
    Shown = 1,
    /// The second pass's: what the first didn't keep and its depth doesn't hide.
    Unhidden = 2,
}

/// The culling and pyramid pipelines, and what a frame culls, kept so a frame culling no more than
/// the last allocates nothing.
pub(crate) struct Culling {
    gather: ComputePipeline,
    choose: ComputePipeline,
    offsets: ComputePipeline,
    write: ComputePipeline,
    meshlets: ComputePipeline,
    /// Builds the pyramid's first level from depth; `pyramid_reduce` builds the rest.
    pyramid_depth: ComputePipeline,
    pyramid_reduce: ComputePipeline,
    runs: Vec<GpuRun>,
    blocks: Vec<[u32; 2]>,
    /// A block of a run each, as `blocks` has them.
    copies: Vec<GpuCopy>,
    levels: Vec<GpuLevel>,
    /// In slot order; the second pass's last.
    views: Vec<GpuView>,
    /// By the draws' run index, its index in `runs`, if culled.
    of_run: Vec<Option<u32>>,
    /// Likewise for the scene's runs.
    of_scene: Vec<Option<u32>>,
    /// Instances and parameter words of `runs`: each view's room.
    instances: u32,
    params_words: u32,
    /// The first pass's push; the second's differs only in view and pyramid. `None` if no run is
    /// culled. `meshlets_pushed` likewise, if any run is culled by meshlets.
    pushed: Option<CullPush>,
    meshlets_pushed: Option<MeshletPush>,
    /// Meshlet words per view the frame planned. Runs past them draw by indices: a mesh drawable
    /// only since the plan, or runs past [`MOST_MESHLET_WORDS`].
    meshlet_budget: u64,
}

impl Culling {
    pub(crate) fn new(device: &std::sync::Arc<Device>) -> Result<Self> {
        let pipeline = |name, entry| ComputePipeline::new(device, name, SPIRV, entry);
        let pyramid = |name, entry| ComputePipeline::new(device, name, PYRAMID_SPIRV, entry);
        Ok(Self {
            gather: ComputePipeline::new(device, "cull gather", GATHER_SPIRV, c"gather_main")?,
            choose: pipeline("cull choose", c"choose_main")?,
            offsets: pipeline("cull offsets", c"offsets_main")?,
            write: pipeline("cull write", c"write_main")?,
            meshlets: ComputePipeline::new(
                device,
                "cull meshlets",
                MESHLETS_SPIRV,
                c"meshlets_main",
            )?,
            pyramid_depth: pyramid("depth pyramid", c"depth_main")?,
            pyramid_reduce: pyramid("depth pyramid levels", c"reduce_main")?,
            runs: Vec::new(),
            blocks: Vec::new(),
            copies: Vec::new(),
            levels: Vec::new(),
            views: Vec::new(),
            of_run: Vec::new(),
            of_scene: Vec::new(),
            instances: 0,
            params_words: 0,
            pushed: None,
            meshlets_pushed: None,
            meshlet_budget: 0,
        })
    }

    pub(super) fn of_run(&self, run: usize) -> Option<usize> {
        Some((*self.of_run.get(run)?)? as usize)
    }

    pub(super) fn of_scene(&self, run: usize) -> Option<usize> {
        Some((*self.of_scene.get(run)?)? as usize)
    }

    pub(super) fn by_meshlets(&self, run: usize) -> bool {
        self.runs[run].meshlets != NO_MESHLETS
    }

    /// Addresses, in the graph buffer at `buffer`, of `views` views' kept instance arrays:
    /// transforms (then `previous` arrays of last frame's), IDs, parameters. Each array holds one
    /// view after another.
    fn kept(&self, buffer: u64, views: u64, previous: u64) -> [u64; 3] {
        let ids = buffer + (views + previous) * u64::from(self.instances) * TRANSFORM;
        [buffer, ids, ids + views * u64::from(self.instances) * ID]
    }
}

/// Last-frame transform arrays kept past the views' own: the first pass's, then the second's.
const PREVIOUS: u64 = 2;

/// Index of pass `slot`'s last-frame transforms among `views` views' transform arrays.
fn previous_of(slot: usize, views: usize) -> u32 {
    (views + usize::from(slot != FIRST)) as u32
}

/// The most a frame's culling takes: runs, instances, blocks of [`BLOCK`] (each run's apart) and
/// parameter words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bounds {
    runs: u64,
    instances: u64,
    blocks: u64,
    params_words: u64,
    /// Per view, [`meshlet_words`] of each run drawn by them, up to [`MOST_MESHLET_WORDS`].
    meshlet_words: u64,
    /// Runs drawn by indices: those without meshlets or past [`MOST_MESHLET_WORDS`].
    by_indices: u64,
    /// Whether the cascades cull and draw by meshlets too, as they do through mesh shaders. Through
    /// vertex shaders, they draw by their levels' indices: a meshlet drawn so shades three corners
    /// for each of [`MAX_MESHLET_TRIANGLES`](crate::packed::MAX_MESHLET_TRIANGLES), its own or
    /// not, where an indexed draw reuses shaded vertices. Depth alone needs no meshlet IDs.
    cascade_meshlets: bool,
}

impl Bounds {
    /// Adds a run of `count` instances of `params_words` words each, drawn by `placed` meshlets as
    /// far as [`MOST_MESHLET_WORDS`] holds, else by indices.
    fn add(&mut self, count: u64, params_words: u64, placed: Option<&PlacedMeshlets>) {
        self.runs += 1;
        self.instances += count;
        self.blocks += count.div_ceil(BLOCK);
        self.params_words += count * params_words;
        match placed {
            Some(placed) => {
                let words = self.meshlet_words + meshlet_words(placed, count);
                self.by_indices += u64::from(words > MOST_MESHLET_WORDS);
                self.meshlet_words = words.min(MOST_MESHLET_WORDS);
            }
            None => self.by_indices += 1,
        }
    }

    /// Per view: kept transforms, IDs and parameters.
    fn kept_bytes(&self) -> u64 {
        self.instances * (TRANSFORM + ID) + self.params_words * 4
    }

    /// Per view: a command per run and level, up to [`MAX_LEVELS`].
    fn commands_bytes(&self) -> u64 {
        self.runs * MAX_LEVELS as u64 * COMMAND
    }

    /// Where the `index`th view with meshlet room has its meshlets, in words: after all `views`
    /// views' commands, view after view.
    fn meshlets_at(&self, views: u64, index: u64) -> u64 {
        views * self.commands_bytes() / 4 + index * self.meshlet_words
    }

    /// Per view: instances' places, then blocks' counts per level.
    fn scratch_bytes(&self) -> u64 {
        self.instances * 4 + self.blocks * MAX_LEVELS as u64 * 4
    }
}

/// Levels and total texels of a `size` view's depth pyramid: halving, rounded up, from half the
/// view down to a texel, as `shaders/pyramid.slang` builds and `shaders/cull.slang` reads it.
fn pyramid(size: [u32; 2]) -> (usize, u64) {
    let mut texels = size.map(|side| side.div_ceil(2));
    let (mut levels, mut total) = (1, 0);
    loop {
        total += u64::from(texels[0]) * u64::from(texels[1]);
        if texels == [1, 1] {
            return (levels, total);
        }
        texels = texels.map(|side| side.div_ceil(2));
        levels += 1;
    }
}

/// A frame's culling in its graph: buffers (a view's room per view), readback and passes. The first
/// pass, for the view's first pass and cascades, precedes their draws. The pyramid's and the second
/// pass follow the view's first pass, before its second.
pub(crate) struct CullPlan {
    views: usize,
    /// Views with meshlet room: all, or the game's view's two passes if the cascades draw by
    /// indices ([`Bounds::cascade_meshlets`]).
    meshlet_views: usize,
    bounds: Bounds,
    /// Culled instances, gathered as one view's are kept.
    gathered: BufferId,
    /// Each view's kept instances, laid out by [`Culling::kept`].
    instances: BufferId,
    /// Each view's commands, then each meshlet view's meshlets: each run's draw and those kept.
    commands: BufferId,
    /// Whether each instance was unhidden, kept across frames; then each view's places and block
    /// counts.
    scratch: BufferId,
    /// What the game's view drew, as `shaders/cull.slang` counts it.
    counts: BufferId,
    gather: PassId,
    choose: PassId,
    offsets: PassId,
    write: PassId,
    meshlets: PassId,
    /// Declared by [`CullPlan::second_pass`].
    second: Option<SecondPass>,
}

/// The game's view's depth pyramid and second culling pass.
struct SecondPass {
    /// The view's depth, and its size.
    depth: ImageId,
    size: [u32; 2],
    pyramid: BufferId,
    /// The passes building the pyramid's levels, in order.
    levels: [Option<PassId>; MOST_PYRAMID_LEVELS],
    choose: PassId,
    offsets: PassId,
    write: PassId,
    meshlets: PassId,
    read: PassId,
}

impl CullPlan {
    /// Declares the culling of runs within `bounds` for the view's two passes, and the cascades if
    /// `cascades`. The first pass runs on the compute queue, before the draws that take it through
    /// [`CullPlan::drawn`]. The second ([`CullPlan::second_pass`]) follows the view's first, on the
    /// graphics queue, as it samples depth.
    pub(crate) fn new(graph: &mut Graph, bounds: Bounds, cascades: bool) -> Self {
        let views = if cascades { MAX_VIEWS } else { 2 };
        let meshlet_views = if bounds.cascade_meshlets { views } else { 2 };
        let count = views as u64;
        // Last frame's transforms, of the gathered instances and the view's passes' kept ones.
        let previous = bounds.instances * TRANSFORM;
        let gathered = graph.buffer("gathered instances", bounds.kept_bytes() + previous);
        let kept = count * bounds.kept_bytes() + PREVIOUS * previous;
        let instances = graph.buffer("culled instances", kept);
        let meshlet_bytes = meshlet_views as u64 * bounds.meshlet_words * 4;
        let commands = graph.buffer(
            "culled draws",
            count * bounds.commands_bytes() + meshlet_bytes,
        );
        let scratch = graph.buffer(
            "culling",
            bounds.instances * 4 + count * bounds.scratch_bytes(),
        );
        let counts = graph.buffer(COUNTS, 16);
        let gather = (graph.compute(label!("cull gather")).on_compute_queue())
            .write_buffer(gathered)
            .add();
        let choose = (graph.compute(label!("cull choose")).on_compute_queue())
            .read_buffer(gathered)
            .write_buffer(scratch)
            .write_buffer(counts)
            .add();
        let offsets = (graph.compute(label!("cull offsets")).on_compute_queue())
            .write_buffer(scratch)
            .write_buffer(commands)
            .write_buffer(counts)
            .add();
        let write = (graph.compute(label!("cull write")).on_compute_queue())
            .read_buffer(gathered)
            .read_buffer(scratch)
            .write_buffer(instances)
            .add();
        let meshlets = (graph.compute(label!("cull meshlets")).on_compute_queue())
            .read_buffer(gathered)
            .read_buffer(scratch)
            .write_buffer(commands)
            .write_buffer(counts)
            .add();
        Self {
            views,
            meshlet_views,
            bounds,
            gathered,
            instances,
            commands,
            scratch,
            counts,
            gather,
            choose,
            offsets,
            write,
            meshlets,
            second: None,
        }
    }

    /// Declares, after the view's first pass, the pyramid from `depth` (`size` pixels), the second
    /// culling pass against it, and the readback of what the view drew. The view's second pass
    /// takes it through [`CullPlan::drawn`].
    ///
    /// # Panics
    ///
    /// - If it's already declared.
    pub(crate) fn second_pass(&mut self, graph: &mut Graph, depth: ImageId, size: [u32; 2]) {
        assert!(self.second.is_none(), "a culling has one second pass");
        let (count, texels) = pyramid(size);
        let pyramid = graph.buffer("depth pyramid", texels * 4);
        let mut levels = [None; MOST_PYRAMID_LEVELS];
        for (i, level) in levels.iter_mut().take(count).enumerate() {
            let pass = graph.compute(label!("depth pyramid"));
            let pass = if i == 0 { pass.sample(depth) } else { pass };
            *level = Some(pass.write_buffer(pyramid).add());
        }
        let choose = (graph.compute(label!("cull choose second")))
            .read_buffer(self.gathered)
            .read_buffer(pyramid)
            .write_buffer(self.scratch)
            .write_buffer(self.counts)
            .add();
        let offsets = (graph.compute(label!("cull offsets second")))
            .write_buffer(self.scratch)
            .write_buffer(self.commands)
            .write_buffer(self.counts)
            .add();
        let write = (graph.compute(label!("cull write second")))
            .read_buffer(self.gathered)
            .read_buffer(self.scratch)
            .write_buffer(self.instances)
            .add();
        let meshlets = (graph.compute(label!("cull meshlets second")))
            .read_buffer(self.gathered)
            .read_buffer(self.scratch)
            .read_buffer(pyramid)
            .write_buffer(self.commands)
            .write_buffer(self.counts)
            .add();
        let read = graph.read_back(label!("read culled counts"), self.counts);
        self.second = Some(SecondPass {
            depth,
            size,
            pyramid,
            levels,
            choose,
            offsets,
            write,
            meshlets,
            read,
        });
    }

    /// The second pass's slot: the last.
    pub(crate) fn second(&self) -> usize {
        self.views - 1
    }

    /// `pass`, reading the kept instances, commands and meshlets.
    pub(crate) fn drawn<'g>(&self, pass: RenderPass<'g>) -> RenderPass<'g> {
        pass.read_buffer(self.instances).indirect(self.commands)
    }

    /// Where view `slot`'s meshlets start among the commands, in words. `None` for a cascade
    /// without meshlet room, which draws by indices alone.
    fn meshlets_at(&self, slot: usize) -> Option<u64> {
        let index = if self.meshlet_views == self.views {
            slot
        } else if slot == FIRST {
            0
        } else if slot == self.second() {
            1
        } else {
            return None;
        };
        Some(self.bounds.meshlets_at(self.views as u64, index as u64))
    }

    /// Whether view `slot` draws culled run `run` of `culling` by its kept meshlets: if it has
    /// meshlet room and the culling kept the run's meshlets.
    pub(super) fn draws_meshlets(&self, slot: usize, culling: &Culling, run: usize) -> bool {
        self.meshlets_at(slot).is_some() && culling.by_meshlets(run)
    }

    /// Whether it culls runs by their meshlets, into a visibility buffer.
    pub(crate) fn by_meshlets(&self) -> bool {
        self.bounds.meshlet_words > 0
    }

    /// Whether it may cull runs by indices, outside a visibility buffer: meshes without meshlets or
    /// not drawable when planned, or meshlets past the frame's room.
    pub(crate) fn by_indices(&self) -> bool {
        self.bounds.by_indices > 0
    }

    /// `pass`, reading the kept instances and meshlets to shade the view from its visibility buffer
    /// ([`MeshPass::shade`]).
    pub(crate) fn shaded<'g>(&self, pass: ComputePass<'g>) -> ComputePass<'g> {
        pass.read_buffer(self.instances).read_buffer(self.commands)
    }
}

/// What shading the view from its visibility buffer reads of the culling: addresses of its runs,
/// commands, and kept transforms and parameters; each view's instance and parameter-word room; and
/// per pass of the view, its slot, its meshlets' start in words, and its last-frame transforms'
/// array index.
pub(super) struct Kept {
    pub(super) runs: u64,
    pub(super) commands: u64,
    pub(super) transforms: u64,
    pub(super) params: u64,
    pub(super) instances: u32,
    pub(super) params_words: u32,
    pub(super) slots: [u32; 2],
    pub(super) meshlets: [u32; 2],
    pub(super) previous: [u32; 2],
}

impl MeshPass {
    /// What `pass` ([`CullPlan::shaded`]) reads of `plan`'s kept instances. No runs if the culling
    /// ran none, as the visibility buffer is empty.
    pub(super) fn kept(&self, plan: &CullPlan, pass: &Compute<'_, '_>) -> Kept {
        let culling = &self.culling;
        let views = plan.views as u64;
        let [transforms, _, params] = culling.kept(pass.address(plan.instances), views, PREVIOUS);
        let slots = [FIRST, plan.second()];
        Kept {
            runs: culling.pushed.map_or(0, |push| push.runs),
            commands: pass.address(plan.commands),
            transforms,
            params,
            instances: culling.instances,
            params_words: culling.params_words,
            slots: slots.map(|slot| slot as u32),
            meshlets: slots.map(|slot| {
                let at = plan.meshlets_at(slot);
                at.expect("the game's view's passes have meshlet room") as u32
            }),
            previous: slots.map(|slot| previous_of(slot, plan.views)),
        }
    }
}

/// A view a frame culls for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CullView {
    pub(crate) clip_from_world: Mat4,
    /// In pixels, across then down; it picks levels of detail.
    pub(crate) size: [u32; 2],
    /// Whether depth clamps at its near side, as a cascade's does, so nearer things draw there,
    /// unculled.
    pub(crate) clamped: bool,
    /// More planes, as [`planes`] has them, that what it keeps reaches inside: a cascade's
    /// [`casters`](crate::shadow::Cascades::casters). [`Vec4::W`] holds all.
    pub(crate) casters: [Vec4; OUTLINE],
}

/// The buffer the game's view's drawn counts go in, which frames read back.
pub(crate) const COUNTS: &str = "culled counts";

/// What the game's view drew, as `shaders/cull.slang` counts it: draws, instances, and the
/// triangles' low and high words.
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

/// `Run` in `shaders/runs.slang`.
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
    meshlets: u32,
    meshlet_draw: u32,
    meshlet_room: u32,
    cones: u32,
    /// Its mesh buffer's address, low and high words.
    vertices: [u32; 2],
}

/// `Copy` in `shaders/gather.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuCopy {
    dest: u32,
    dest_params: u32,
    count: u32,
    params_words: u32,
    source: u32,
    source_params: u32,
}

/// `Push` in `shaders/gather.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GatherPush {
    copies: u64,
    frame_transforms: u64,
    frame_ids: u64,
    frame_params: u64,
    scene: u64,
    order: u64,
    transforms: u64,
    ids: u64,
    params: u64,
    copy_count: u32,
    groups: u32,
    /// The frame number, low and high words, compared with a record's last change.
    frame: [u32; 2],
    instances: u32,
    unused: u32,
}

/// `Level` in `shaders/culling.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuLevel {
    start: u32,
    count: u32,
    error: f32,
    meshlet_first: u32,
    meshlet_count: u32,
}

/// `View` in `shaders/culling.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuView {
    /// [`planes`], then [`CullView::casters`].
    planes: [Vec4; PLANES],
    w: Vec4,
    x: Vec4,
    y: Vec4,
    z: Vec4,
    w_slope: f32,
    pixels_per_unit: f32,
    slot: u32,
    keep: u32,
    width: u32,
    height: u32,
    pyramid_levels: u32,
    meshlets: u32,
    eye: Vec4,
    /// For the view's passes, where last frame's kept transforms go ([`previous_of`]);
    /// [`NO_PREVIOUS`] for a cascade.
    previous: u32,
    unused: [u32; 3],
}

/// A view's planes: `PLANES` in `shaders/culling.slang`.
const PLANES: usize = 6 + OUTLINE;

/// `NO_PREVIOUS` in `shaders/culling.slang`.
const NO_PREVIOUS: u32 = u32::MAX;

impl GpuView {
    fn new(view: &CullView, plan: &CullPlan, slot: usize, keep: Keep) -> Self {
        let clip = &view.clip_from_world;
        let lod = LodView::new(clip, view.size);
        let [width, height] = view.size;
        Self {
            planes: {
                let mut all = [Vec4::W; PLANES];
                let (sides, casters) = all.split_at_mut(6);
                sides.copy_from_slice(&planes(clip, view.clamped));
                casters.copy_from_slice(&view.casters);
                all
            },
            w: lod.w,
            x: clip.row(0),
            y: clip.row(1),
            z: clip.row(2),
            w_slope: lod.w_slope,
            pixels_per_unit: lod.pixels_per_unit,
            slot: slot as u32,
            keep: keep as u32,
            width,
            height,
            pyramid_levels: pyramid(view.size).0 as u32,
            meshlets: plan.meshlets_at(slot).map_or(NO_MESHLETS, |at| at as u32),
            eye: eye(clip),
            previous: match keep {
                Keep::All => NO_PREVIOUS,
                Keep::Shown | Keep::Unhidden => previous_of(slot, plan.views),
            },
            unused: [0; 3],
        }
    }
}

/// The eye of `clip_from_world`. Under perspective: the camera, where clip W is 0, and 1.
/// Otherwise: the way it looks, and 0; along it clip X and Y stay put and depth falls, as reversed
/// depth falls away from the camera.
fn eye(clip_from_world: &Mat4) -> Vec4 {
    let [x, y, z, w] = [0, 1, 2, 3].map(|row| clip_from_world.row(row));
    if w.truncate() != Vec3::ZERO {
        // Where clip X, Y and W are all 0.
        let rows = Mat3::from_cols(x.truncate(), y.truncate(), w.truncate()).transpose();
        let at = rows.inverse() * -Vec3::new(x.w, y.w, w.w);
        return at.extend(1.0);
    }
    let way = x.truncate().cross(y.truncate()).normalize_or_zero();
    let way = if way.dot(z.truncate()) > 0.0 {
        -way
    } else {
        way
    };
    way.extend(0.0)
}

/// `Push` in `shaders/meshlets.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct MeshletPush {
    runs: u64,
    blocks: u64,
    levels: u64,
    views: u64,
    transforms: u64,
    scratch: u64,
    counts: u64,
    pyramid: u64,
    bounds: u64,
    commands: u64,
    run_count: u32,
    block_count: u32,
    instances: u32,
    unused: u32,
}

/// `Push` in `shaders/cull.slang`, which says why it takes each address.
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
    pyramid: u64,
    run_count: u32,
    block_count: u32,
    instances: u32,
    params_words: u32,
}

/// `Push` in `shaders/pyramid.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct PyramidPush {
    pyramid: u64,
    source: u32,
    source_width: u32,
    source_height: u32,
    target: u32,
    target_width: u32,
    target_height: u32,
    depth: u32,
    unused: u32,
}

/// The planes of `clip_from_world`'s view in Vulkan's clip space: a point is inside where its dot
/// product with a plane's XYZ, plus W, is at least 0. The sides, the near (depth 1; none if
/// `clamped`), then the far (depth 0), which holds everything when seeing to infinity. Gribb and
/// Hartmann, "Fast Extraction of Viewing Frustum Planes from the World-View-Projection Matrix".
fn planes(clip_from_world: &Mat4, clamped: bool) -> [Vec4; 6] {
    let [x, y, z, w] = [0, 1, 2, 3].map(|row| clip_from_world.row(row));
    let near = if clamped { Vec4::W } else { w - z };
    [w + x, w - x, w + y, w - y, near, z]
}

impl MeshPass {
    /// The most culling `draws` and the scene takes ([`Bounds::add`]): their runs with opaque
    /// shaders, and the meshlets of those drawn by them, of meshes drawable now. `None` if no
    /// shader is opaque.
    pub(crate) fn cull_bounds(&self, draws: &Draws) -> Option<Bounds> {
        let mut bounds = Bounds {
            cascade_meshlets: self.device.mesh_shaders(),
            ..Bounds::default()
        };
        let frame = (draws.runs.iter()).map(|run| (run.mesh, run.shader, run.instances.len()));
        let scene = self.scene.runs().iter();
        let scene = scene.map(|run| (run.mesh, run.shader, run.count as usize));
        for (mesh, shader, count) in frame.chain(scene) {
            let Some(Some(shader)) = self.shaders.get(shader as usize) else {
                continue;
            };
            if shader.writes_depth {
                let mesh = self.meshes.get(mesh as usize).and_then(Held::ready);
                let placed = mesh.and_then(|mesh| mesh.meshlets.as_ref());
                let placed = placed.filter(|_| shader.meshlets.is_some());
                bounds.add(count as u64, (shader.params_size / 4) as u64, placed);
            }
        }
        (bounds.runs > 0).then_some(bounds)
    }

    /// Culls, as `plan` declared, the runs of `draws` and the scene with drawable meshes and opaque
    /// shaders, for the first pass: the view (`seen`) and `cascades`. It gathers their instances
    /// first (`uploaded` for the draws, the scene's table for its own) and records the frame's next
    /// passes. Returns whether it culled any run; if not, the readback counts nothing.
    /// [`MeshPass::cull_second`] culls for the second pass.
    ///
    /// # Panics
    ///
    /// - If `cascades` aren't as many as `plan` was declared for.
    /// - If `draws` aren't what bounded it.
    pub(crate) fn cull(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &CullPlan,
        draws: &Draws,
        uploaded: Uploaded,
        seen: CullView,
        cascades: &[CullView],
    ) -> Result<bool> {
        let _zone = gg_core::zone!();
        assert_eq!(
            2 + cascades.len(),
            plan.views,
            "a culling's views are those it was planned for"
        );
        let culling = &mut self.culling;
        culling.runs.clear();
        culling.blocks.clear();
        culling.copies.clear();
        culling.levels.clear();
        culling.views.clear();
        culling.of_run.clear();
        culling.of_scene.clear();
        culling.pushed = None;
        culling.meshlets_pushed = None;
        culling.meshlet_budget = plan.bounds.meshlet_words;
        let mut totals = Totals::default();
        // The scene's first, so its instances' gathered places, which keep what was unhidden, don't
        // move as the draws change.
        for run in self.scene.runs() {
            let source = Source::Scene { first: run.first };
            let count = run.count as usize;
            let culled = culling.add(&self.meshes, &self.shaders, run, count, source, &mut totals);
            culling.of_scene.push(culled);
        }
        for run in &draws.runs {
            let source = Source::Frame {
                first: run.instances.start as u32,
                params: (run.params.start / 4) as u32,
            };
            let count = run.instances.len();
            let culled = culling.add(&self.meshes, &self.shaders, run, count, source, &mut totals);
            culling.of_run.push(culled);
        }
        let Totals {
            instances,
            params_words,
            meshlet_words,
            ..
        } = totals;
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
            drop(frame.compute(plan.gather));
            drop(frame.compute(plan.choose));
            drop(frame.compute(plan.offsets));
            drop(frame.compute(plan.write));
            drop(frame.compute(plan.meshlets));
            return Ok(false);
        }
        // In slot order: the view's first pass, the cascades, then its second.
        culling
            .views
            .push(GpuView::new(&seen, plan, FIRST, Keep::Shown));
        let cascades = cascades.iter().enumerate();
        (culling.views)
            .extend(cascades.map(|(i, cascade)| GpuView::new(cascade, plan, 1 + i, Keep::All)));
        let second = GpuView::new(&seen, plan, plan.second(), Keep::Unhidden);
        culling.views.push(second);
        let [kept_transforms, kept_ids, kept_params] =
            culling.kept(frame.address(plan.instances), plan.views as u64, PREVIOUS);
        // Laid out as one view's kept instances, each run's at its `output`, with last frame's
        // transforms.
        let [transforms, ids, params] = culling.kept(frame.address(plan.gathered), 1, 1);
        let (commands, scratch, counts) = (
            frame.address(plan.commands),
            frame.address(plan.scratch),
            frame.address(plan.counts),
        );
        let [scene, order] = self.scene.addresses();
        let number = frame.number();
        let mut address = |bytes: &[u8]| -> Result<u64> { Ok(frame.upload(bytes)?.address()) };
        let groups = block_count.min(MOST_GROUPS) as u32;
        let gather = GatherPush {
            copies: address(bytemuck::cast_slice(&culling.copies))?,
            frame_transforms: uploaded.transforms,
            frame_ids: uploaded.ids,
            frame_params: uploaded.params,
            scene,
            order,
            transforms,
            ids,
            params,
            copy_count: block_count as u32,
            groups,
            frame: halves(number),
            instances,
            unused: 0,
        };
        let push = CullPush {
            runs: address(bytemuck::cast_slice(&culling.runs))?,
            blocks: address(bytemuck::cast_slice(&culling.blocks))?,
            levels: address(bytemuck::cast_slice(&culling.levels))?,
            views: address(bytemuck::cast_slice(&culling.views))?,
            transforms,
            ids,
            params,
            kept_transforms,
            kept_ids,
            kept_params,
            commands,
            scratch,
            counts,
            // The second pass's alone, once the pyramid is built.
            pyramid: 0,
            run_count: run_count as u32,
            block_count: block_count as u32,
            instances,
            params_words,
        };
        let mut pass = frame.compute(plan.gather);
        pass.set_pipeline(&culling.gather);
        pass.push_constants(bytemuck::bytes_of(&gather));
        pass.dispatch([groups, 1, 1]);
        drop(pass);
        // Every view but the second pass's, the last.
        culling.dispatch(
            frame,
            [plan.choose, plan.offsets, plan.write],
            &push,
            plan.views - 1,
        );
        culling.pushed = Some(push);
        let mut pass = frame.compute(plan.meshlets);
        // The view's first pass, then the cascades with meshlet room.
        let meshlet_views = plan.meshlet_views - 1;
        if meshlet_words > 0 {
            let meshlets = MeshletPush {
                runs: push.runs,
                blocks: push.blocks,
                levels: push.levels,
                views: push.views,
                transforms,
                scratch,
                counts,
                pyramid: 0,
                bounds: self.meshlet_table.address(),
                commands,
                run_count: push.run_count,
                block_count: push.block_count,
                instances,
                unused: 0,
            };
            culling.meshlets_dispatch(&mut pass, &meshlets, meshlet_views);
            culling.meshlets_pushed = Some(meshlets);
        }
        Ok(true)
    }

    /// Builds the view's depth pyramid from its first pass's depth, and culls against it, for the
    /// second pass, the runs [`MeshPass::cull`] culled, as `plan` declared. Records the frame's
    /// next passes and the readback of what the view drew.
    ///
    /// # Panics
    ///
    /// - If `plan` has no second pass declared.
    pub(crate) fn cull_second(&self, frame: &mut Frame<'_>, plan: &CullPlan) -> Result<()> {
        let _zone = gg_core::zone!();
        let second = (plan.second.as_ref()).expect("a culling's second pass, declared");
        let culling = &self.culling;
        let levels = second.levels.iter().map_while(|&level| level);
        let Some(push) = culling.pushed else {
            // Its passes are the frame's all the same.
            for level in levels {
                drop(frame.compute(level));
            }
            drop(frame.compute(second.choose));
            drop(frame.compute(second.offsets));
            drop(frame.compute(second.write));
            drop(frame.compute(second.meshlets));
            frame.read_back(second.read)?;
            return Ok(());
        };
        let pyramid = frame.address(second.pyramid);
        let (mut source, mut source_size) = (0, second.size);
        let mut target = 0;
        for (i, level) in levels.enumerate() {
            let mut pass = frame.compute(level);
            let target_size = source_size.map(|side| side.div_ceil(2));
            let depth = if i == 0 {
                pass.set_pipeline(&culling.pyramid_depth);
                pass.sampled(second.depth)
            } else {
                pass.set_pipeline(&culling.pyramid_reduce);
                0
            };
            let built = PyramidPush {
                pyramid,
                source,
                source_width: source_size[0],
                source_height: source_size[1],
                target,
                target_width: target_size[0],
                target_height: target_size[1],
                depth,
                unused: 0,
            };
            pass.push_constants(bytemuck::bytes_of(&built));
            let [across, down] = target_size.map(|side| side.div_ceil(PYRAMID_GROUP));
            pass.dispatch([across, down, 1]);
            (source, source_size) = (target, target_size);
            target += target_size[0] * target_size[1];
        }
        // The second pass's view alone, the last.
        let view = (plan.second() * size_of::<GpuView>()) as u64;
        let push = CullPush {
            views: push.views + view,
            pyramid,
            ..push
        };
        let passes = [second.choose, second.offsets, second.write];
        culling.dispatch(frame, passes, &push, 1);
        let mut pass = frame.compute(second.meshlets);
        if let Some(meshlets) = culling.meshlets_pushed {
            let meshlets = MeshletPush {
                views: meshlets.views + view,
                pyramid,
                ..meshlets
            };
            culling.meshlets_dispatch(&mut pass, &meshlets, 1);
        }
        drop(pass);
        frame.read_back(second.read)?;
        Ok(())
    }

    /// Draws what view `slot` of `plan` kept of culled run `run`, with `push`'s mesh, as `way`
    /// draws: a draw per level, as its commands say, or its kept meshlets
    /// ([`MeshPass::by_meshlets`]).
    pub(super) fn draw_culled(
        &self,
        pass: &mut Pass<'_, '_>,
        mesh: &GpuMesh,
        push: Push,
        (plan, slot): (&CullPlan, usize),
        run: usize,
        way: Way,
    ) {
        let culling = &self.culling;
        let (index, by_meshlets) = (run as u32, self.by_meshlets((plan, slot), run, way));
        let run = &culling.runs[run];
        let [transforms, ids, params] =
            culling.kept(pass.address(plan.instances), plan.views as u64, PREVIOUS);
        // The view's passes read their last-frame transforms past every view's, `previous`
        // transforms on; a cascade's read their own.
        let previous = if slot == FIRST || slot == plan.second() {
            (previous_of(slot, plan.views) - slot as u32) * culling.instances
        } else {
            0
        };
        let slot = slot as u64;
        let first = slot * u64::from(culling.instances) + u64::from(run.output);
        let params = if run.params_words == 0 {
            0
        } else {
            params + (slot * u64::from(culling.params_words) + u64::from(run.params_output)) * 4
        };
        let push = Push {
            transforms: transforms + first * TRANSFORM,
            ids: ids + first * ID,
            params,
            previous,
            ..push
        };
        if by_meshlets {
            // Its draw's eight words, then the meshlets it keeps, which the shader reads.
            let at = plan
                .meshlets_at(slot as usize)
                .expect("a view drawing meshlets has room");
            let draw = (at + u64::from(run.meshlet_draw)) * 4;
            // Which run, and which of the game's view's passes, as `Push::draw` says.
            let second = slot as usize == plan.second();
            let push = Push {
                meshlets: pass.address(plan.commands) + draw,
                draw: if way == Way::Visibility {
                    1 + 2 * index + u32::from(second)
                } else {
                    0
                },
                ..push
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            if self.device.mesh_shaders() {
                pass.draw_mesh_tasks_indirect(plan.commands, draw + 16);
            } else {
                pass.draw_indirect(plan.commands, draw);
            }
            return;
        }
        pass.push_constants(bytemuck::bytes_of(&push));
        let runs = culling.runs.len() as u64;
        let commands = (slot * runs * MAX_LEVELS as u64 + u64::from(run.commands)) * COMMAND;
        for level in 0..u64::from(run.level_count) {
            pass.draw_indexed_indirect(&mesh.indices, plan.commands, commands + level * COMMAND);
        }
    }
}

/// Where the gather copies a culled run's instances from: the draws' first instance and parameter
/// word, or the scene's first instance.
#[derive(Clone, Copy)]
enum Source {
    Frame { first: u32, params: u32 },
    Scene { first: u32 },
}

/// Running totals of the culled runs.
#[derive(Default)]
struct Totals {
    instances: u32,
    params_words: u32,
    commands: u32,
    meshlet_words: u32,
}

impl Totals {
    /// Takes a run's meshlet words per view, if within the planned `budget`, and returns its draw's
    /// start among them and its meshlet room. Past the budget it takes nothing, and the run draws
    /// by indices.
    fn take_meshlets(
        &mut self,
        placed: &PlacedMeshlets,
        count: u32,
        budget: u64,
    ) -> Option<(u32, u32)> {
        let words = meshlet_words(placed, u64::from(count));
        let taken = u64::from(self.meshlet_words) + words;
        if taken > budget {
            return None;
        }
        // Within the budget, which is within `MOST_MESHLET_WORDS`.
        let draw = std::mem::replace(&mut self.meshlet_words, taken as u32);
        Some((draw, meshlet_room(placed, u64::from(count)) as u32))
    }
}

/// A run that may be culled.
trait Culled {
    fn mesh(&self) -> u32;
    fn shader(&self) -> u32;
    fn faces(&self) -> Faces;
}

impl Culled for crate::draws::Run {
    fn mesh(&self) -> u32 {
        self.mesh
    }

    fn shader(&self) -> u32 {
        self.shader
    }

    fn faces(&self) -> Faces {
        self.faces
    }
}

impl Culled for SceneRun {
    fn mesh(&self) -> u32 {
        self.mesh
    }

    fn shader(&self) -> u32 {
        self.shader
    }

    fn faces(&self) -> Faces {
        self.faces
    }
}

impl Culling {
    /// Records a culling pass's `passes` (choose, offsets, write), each pushing `push`, for its
    /// first `views` views.
    fn dispatch(&self, frame: &mut Frame<'_>, passes: [PassId; 3], push: &CullPush, views: usize) {
        let groups = u64::from(push.block_count).min(MOST_GROUPS) as u32;
        let runs = u64::from(push.run_count).div_ceil(BLOCK) as u32;
        let [choose, offsets, write] = passes;
        let views = views as u32;
        for (pass, pipeline, across) in [
            (choose, &self.choose, groups),
            (offsets, &self.offsets, runs),
            (write, &self.write, groups),
        ] {
            let mut pass = frame.compute(pass);
            pass.set_pipeline(pipeline);
            pass.push_constants(bytemuck::bytes_of(push));
            pass.dispatch([across, views, 1]);
        }
    }

    /// Records a culling pass's meshlet culling, pushing `push`, for its first `views` views.
    fn meshlets_dispatch(&self, pass: &mut Compute<'_, '_>, push: &MeshletPush, views: usize) {
        let groups = u64::from(push.block_count).min(MOST_GROUPS) as u32;
        pass.set_pipeline(&self.meshlets);
        pass.push_constants(bytemuck::bytes_of(push));
        pass.dispatch([groups, views as u32, 1]);
    }

    /// Adds `run`, of `count` instances at `source`, to the culled runs with its blocks and copies,
    /// if its mesh is drawable and its shader opaque, and returns its index; else `None`. If its
    /// mesh has meshlets and its shader draws them, it's drawn by them, given room the frame
    /// planned.
    ///
    /// # Panics
    ///
    /// - If its instances don't fit Vulkan's 32-bit count.
    /// - If the scene's instances take more parameters than a [`MeshDraw`] holds.
    fn add(
        &mut self,
        meshes: &[Held<GpuMesh>],
        shaders: &[Option<GpuShader>],
        run: &impl Culled,
        count: usize,
        source: Source,
        totals: &mut Totals,
    ) -> Option<u32> {
        let mesh = meshes.get(run.mesh() as usize).and_then(Held::ready)?;
        let shader = shaders
            .get(run.shader() as usize)
            .and_then(Option::as_ref)?;
        if !shader.writes_depth {
            return None;
        }
        let GpuMesh { lods, .. } = mesh;
        let count =
            u32::try_from(count).expect("a run's instances fit the 32-bit count Vulkan takes");
        // Drawn by meshlets where room was planned: its draw's start among each view's meshlets,
        // and its room.
        let placed = (mesh.meshlets.as_ref()).filter(|_| shader.meshlets.is_some());
        let drawn =
            placed.and_then(|placed| totals.take_meshlets(placed, count, self.meshlet_budget));
        let placed = placed.filter(|_| drawn.is_some());
        let words = (shader.params_size / 4) as u32;
        let (first_block, block_count) = (self.blocks.len(), count.div_ceil(BLOCK as u32));
        let run_index = self.runs.len() as u32;
        let Totals {
            instances,
            params_words,
            commands,
            meshlet_words,
        } = *totals;
        for block in 0..block_count {
            let first = block * BLOCK as u32;
            self.blocks.push([run_index, first]);
            let (source, source_params) = match source {
                Source::Frame { first: at, params } => (at + first, params + first * words),
                Source::Scene { first: at } => {
                    assert!(
                        shader.params_size <= MeshDraw::MAX_PARAMS,
                        "a scene's instance takes parameters of up to {} bytes",
                        MeshDraw::MAX_PARAMS
                    );
                    (at + first, SCENE)
                }
            };
            self.copies.push(GpuCopy {
                dest: instances + first,
                dest_params: params_words + first * words,
                count: (count - first).min(BLOCK as u32),
                params_words: words,
                source,
                source_params,
            });
        }
        let first_level = self.levels.len();
        let meshlets_of = |level: usize| placed.map_or([0, 0], |placed| placed.levels[level]);
        self.levels
            .extend(lods.levels.iter().enumerate().map(|(i, lod)| {
                let [meshlet_first, meshlet_count] = meshlets_of(i);
                GpuLevel {
                    start: lod.start,
                    count: lod.count,
                    error: lod.error,
                    meshlet_first,
                    meshlet_count,
                }
            }));
        // A run drawn by its levels' indices reads neither.
        let (meshlet_draw, room) = drawn.unwrap_or((meshlet_words, 0));
        // Gathered as one view's are kept: each run's at its `output`.
        self.runs.push(GpuRun {
            first: instances,
            count,
            params: params_words,
            params_words: words,
            sphere: lods.center.extend(lods.radius),
            levels: first_level as u32,
            level_count: lods.levels.len() as u32,
            output: instances,
            params_output: params_words,
            commands,
            blocks: first_block as u32,
            block_count,
            meshlets: placed.map_or(NO_MESHLETS, |placed| placed.first),
            meshlet_draw,
            meshlet_room: room,
            cones: u32::from(run.faces() != Faces::Both),
            vertices: crate::halves(mesh.vertices.address()),
        });
        // Its meshlets' words were taken above.
        *totals = Totals {
            instances: instances + count,
            params_words: params_words + count * words,
            commands: commands + lods.levels.len() as u32,
            meshlet_words,
        };
        Some(run_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Projection, Viewpoint};
    use gg_core::math::{DVec3, Quat, Vec3};

    fn inside(planes: &[Vec4; 6], point: Vec3) -> bool {
        planes
            .iter()
            .all(|plane| plane.truncate().dot(point) + plane.w >= 0.0)
    }

    /// A perspective's planes hold what's ahead within its angle, however far, and nothing behind
    /// or nearer than its near plane; a clamped view's hold what's nearer too.
    #[test]
    fn a_views_planes_hold_what_it_sees() {
        let viewpoint = Viewpoint {
            position: DVec3::ZERO,
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

    /// A perspective view's eye is its camera; an orthographic one's is the way it faces, however
    /// turned.
    #[test]
    fn a_view_sees_from_its_camera_or_looks_its_way() {
        let rotation = Quat::from_euler(gg_core::math::EulerRot::YXZ, 0.7, -0.4, 0.2);
        let position = Vec3::new(3.0, -2.0, 5.5);
        let seen = |projection| {
            let viewpoint = Viewpoint {
                position: position.as_dvec3(),
                rotation,
                projection,
            };
            eye(&viewpoint.view_projection(1.5))
        };
        let perspective = seen(Projection::Perspective {
            fov: std::f32::consts::FRAC_PI_3,
            near: 0.1,
        });
        assert!(perspective.w == 1.0, "{perspective}");
        assert!(
            perspective.truncate().distance(position) < 1e-4,
            "{perspective}"
        );
        let orthographic = seen(Projection::Orthographic {
            height: 4.0,
            near: -10.0,
            far: 10.0,
        });
        let ahead = rotation * Vec3::NEG_Z;
        assert!(orthographic.w == 0.0, "{orthographic}");
        assert!(
            orthographic.truncate().distance(ahead) < 1e-5,
            "{orthographic}"
        );
    }

    /// Meshlet room is planned per view up to [`MOST_MESHLET_WORDS`]. Culling draws by meshlets the
    /// runs that fit, in turn, and the rest by indices, taking no room. The plan counts as by
    /// indices the runs without meshlets and each past the full room.
    #[test]
    fn runs_past_a_views_room_for_meshlets_are_drawn_by_their_indices() {
        let placed = |most| PlacedMeshlets {
            first: 0,
            levels: vec![[0, most]],
            most,
        };
        let (small, large) = (placed(2), placed(64));
        // Each a draw and 2 words for each of 3 instances' 2 meshlets.
        let small_words = MESHLET_DRAW + 2 * 6;
        let mut bounds = Bounds::default();
        bounds.add(3, 1, Some(&small));
        bounds.add(3, 1, Some(&small));
        assert_eq!(bounds.by_indices, 0);
        bounds.add(5, 1, None);
        assert_eq!(bounds.meshlet_words, 2 * small_words, "each run's own");
        assert_eq!(bounds.by_indices, 1, "the run without meshlets");
        // Instances enough of the large mesh for the most meshlets a draw takes.
        let many = (MOST_MESHLETS / 64) as u32;
        let runs = [(&small, 3), (&large, many), (&small, 3), (&large, many)];
        let mut bounds = Bounds::default();
        for &(placed, count) in &runs {
            bounds.add(u64::from(count), 1, Some(placed));
        }
        assert_eq!(bounds.meshlet_words, MOST_MESHLET_WORDS);
        assert_eq!(bounds.by_indices, 3, "each run once the room is full");
        let mut totals = Totals::default();
        let drawn =
            runs.map(|(placed, count)| totals.take_meshlets(placed, count, bounds.meshlet_words));
        let first = small_words as u32;
        assert_eq!(drawn, [Some((0, 6)), None, Some((first, 6)), None]);
        assert_eq!(u64::from(totals.meshlet_words), 2 * small_words);
    }

    /// The readback splits into draws, instances and 64-bit triangles.
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

    /// A depth pyramid halves the view, rounding up, down to a texel, and counts all levels'
    /// texels.
    #[test]
    fn a_depth_pyramid_halves_the_view_down_to_a_texel() {
        // 640 by 360, 320 by 180, ..., 3 by 2, 2 by 1, 1 by 1: 11 levels.
        let texels = [
            640 * 360,
            320 * 180,
            160 * 90,
            80 * 45,
            40 * 23,
            20 * 12,
            10 * 6,
            5 * 3,
            3 * 2,
            2,
            1,
        ];
        assert_eq!(pyramid([1280, 720]), (11, texels.iter().sum()));
        assert_eq!(pyramid([1, 1]), (1, 1), "a texel already");
        assert_eq!(pyramid([2, 1]), (1, 1));
        assert_eq!(pyramid([3, 1]), (2, 2 + 1));
        assert_eq!(
            pyramid([u32::MAX; 2]).0,
            MOST_PYRAMID_LEVELS,
            "the most levels a view's sides make"
        );
    }
}
