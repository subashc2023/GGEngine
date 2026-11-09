//! Culling a frame's opaque runs on the GPU, its draws' and its scene's, for each view that draws
//! them: the game's view in each of its two passes, against what it sees and, in its second, the
//! depth pyramid its first pass's depth builds, and the shadows' cascades; and picking each
//! instance's level of detail there, as `shaders/cull.slang` says, once `shaders/gather.slang` has
//! gathered their instances; then the meshlets of the instances kept of a run whose mesh has them
//! and whose shader draws them, as `shaders/meshlets.slang` says; and the indirect draws of what
//! each view keeps, a draw for each run and level, whatever its instances, or, of a run drawn by its
//! meshlets, one of all it keeps of them, through mesh shaders or vertex shaders.

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::label;
use gg_core::math::{Mat3, Mat4, Vec3, Vec4};
use gg_gpu::{
    BufferId, Compute, ComputePass, ComputePipeline, Device, Faces, Frame, Graph, ImageId, Pass,
    PassId, RenderPass,
};

use super::scene::SceneRun;
use super::{
    GpuMesh, GpuShader, Held, LodView, MAX_LEVELS, MeshPass, PlacedMeshlets, Push, Uploaded,
};
use crate::shadow::CASCADES;
use crate::{Draws, MeshDraw, halves};

pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cull.spv"));
pub(crate) static GATHER_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gather.spv"));
pub(crate) static PYRAMID_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pyramid.spv"));
pub(crate) static MESHLETS_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/meshlets.spv"));

/// What a run drawn by its levels' indices has for its first meshlet: `NO_MESHLETS` in
/// `shaders/culling.slang`.
const NO_MESHLETS: u32 = u32::MAX;

/// How many workgroups across a draw of meshlets through a mesh shader takes, as many rows down as
/// they need: `ACROSS` in `shaders/meshlets.slang` and `shaders/model.slang`.
const MESHLET_ACROSS: u64 = 32_768;

/// The most meshlets a run's draw in a view takes: as many workgroups as `VK_EXT_mesh_shader` lets
/// a draw through a mesh shader take on any device, its least `maxMeshWorkGroupTotalCount`, as
/// `MESHLET_ACROSS` across and 128 rows down. Those a view keeps past it are left out, as of a run
/// of thousands of instances of a mesh of thousands of meshlets, all in view at their finest.
const MOST_MESHLETS: u64 = 1 << 22;

// Within the least `maxMeshWorkGroupCount` across and down `VK_EXT_mesh_shader` allows.
const _: () = assert!(MESHLET_ACROSS <= 65_535 && MOST_MESHLETS / MESHLET_ACROSS <= 65_535);

/// The words of a run's draw of its meshlets in a view, before the meshlets it keeps, two words
/// each: as `offsets_main` in `shaders/cull.slang` lays them out, a `VkDrawIndirectCommand`, a
/// `VkDrawMeshTasksIndirectCommandEXT` and how many it kept.
const MESHLET_DRAW: u64 = 8;

/// How many meshlets a run of `count` instances of a mesh whose meshlets are `placed` has room for
/// in each view: every meshlet of its largest level, of every instance, up to [`MOST_MESHLETS`].
fn meshlet_room(placed: &PlacedMeshlets, count: u64) -> u64 {
    (count * u64::from(placed.most)).min(MOST_MESHLETS)
}

/// The words a run of `count` instances of a mesh whose meshlets are `placed` takes in each view's
/// meshlets: its draw, and its [`meshlet_room`].
fn meshlet_words(placed: &PlacedMeshlets, count: u64) -> u64 {
    MESHLET_DRAW + 2 * meshlet_room(placed, count)
}

/// The most words of meshlets a frame plans in each view, past which the runs that would be drawn
/// by their meshlets are drawn by their levels' indices instead, without their meshlets culled or a
/// visibility buffer: room for one run's draw at its most, so that any run alone is drawn by its
/// meshlets, which with its draw is 32 MiB a view, 192 MiB in the six a frame with shadows culls for.
/// Room is planned for every meshlet of every instance, not for what's kept, so without a ceiling
/// the room of a few runs of many instances, as of foliage, outgrows what the GPU can make, and a
/// frame that can't make its buffers ends the renderer. Raising it costs 8 bytes a meshlet in each
/// view whether it's kept or not, and the graph rounds the buffer of commands it shares up to a
/// power of two: room for a second such run would take the six views' 256 MiB to 512 MiB.
const MOST_MESHLET_WORDS: u64 = MESHLET_DRAW + 2 * MOST_MESHLETS;

/// A copy's `source_params` for one from the scene: `SCENE` in `shaders/gather.slang`.
const SCENE: u32 = u32::MAX;

/// How many of a run's instances a workgroup takes: `BLOCK` in `shaders/cull.slang`.
const BLOCK: u64 = 64;

/// The most workgroups a dispatch takes across, the least `maxComputeWorkGroupCount` Vulkan
/// allows: past it, each takes blocks that many apart. `MOST_GROUPS` in `shaders/cull.slang`.
const MOST_GROUPS: u64 = 65_535;

/// The bytes of a `VkDrawIndexedIndirectCommand`.
const COMMAND: u64 = 20;

/// The bytes of an instance's transform and of its ID.
const TRANSFORM: u64 = size_of::<Mat4>() as u64;
const ID: u64 = size_of::<[u32; 2]>() as u64;

/// The side of the workgroups that build the depth pyramid, a texel each: `numthreads` in
/// `shaders/pyramid.slang`.
const PYRAMID_GROUP: u32 = 8;

/// The most levels a depth pyramid has: one for each halving of a side of a view of 2^32 pixels.
const MOST_PYRAMID_LEVELS: usize = 32;

/// The slot among the views a frame culls for of the game's view's first pass, which the shadows'
/// cascades follow, each at 1 past its index, if the frame draws them, and then the view's second
/// pass ([`CullPlan::second`]): `FIRST` in `shaders/cull.slang`.
pub(crate) const FIRST: usize = 0;

/// The most views a frame culls for: the game's view's two passes and the shadows' cascades.
pub(crate) const MAX_VIEWS: usize = 2 + CASCADES;

/// What a view keeps of what reaches inside its planes: `KEEP_ALL`, `KEEP_SHOWN` and
/// `KEEP_UNHIDDEN` in `shaders/cull.slang`, which says what each is.
#[derive(Clone, Copy)]
#[repr(u32)]
enum Keep {
    /// A shadow cascade's: all of it.
    All = 0,
    /// The game's view's first pass: what the last frame found unhidden.
    Shown = 1,
    /// The game's view's second pass: what the first didn't keep that its depth doesn't hide.
    Unhidden = 2,
}

/// The compute pipelines that cull and build the depth pyramid, and what a frame culls, kept
/// between frames, so that one that culls no more than the last allocates nothing.
pub(crate) struct Culling {
    gather: ComputePipeline,
    choose: ComputePipeline,
    offsets: ComputePipeline,
    write: ComputePipeline,
    meshlets: ComputePipeline,
    /// What builds the depth pyramid's first level, from the view's depth, and each after it.
    pyramid_depth: ComputePipeline,
    pyramid_reduce: ComputePipeline,
    runs: Vec<GpuRun>,
    blocks: Vec<[u32; 2]>,
    /// What the gather copies: a block of a run each, as `blocks` has them.
    copies: Vec<GpuCopy>,
    levels: Vec<GpuLevel>,
    /// Every view, in its slot: the first pass's, then the second's, last.
    views: Vec<GpuView>,
    /// For each run of the frame's draws, by its index, its index in `runs`, if it's culled.
    of_run: Vec<Option<u32>>,
    /// For each of the scene's runs, by its index, its index in `runs`, if it's culled.
    of_scene: Vec<Option<u32>>,
    /// The instances of `runs`, and the words of their parameters: what each view keeps room for.
    instances: u32,
    params_words: u32,
    /// What the first pass's culling pushed, which the second's pushes but for its view and the
    /// pyramid; `None` in a frame that culls no run. Its meshlets', if it culls any run by them.
    pushed: Option<CullPush>,
    meshlets_pushed: Option<MeshletPush>,
    /// The words of each view's meshlets the frame plans, which a run drawn by its meshlets past
    /// them is drawn by its levels' indices instead, as of a mesh that came to be drawable after
    /// the frame planned its graph, or of runs whose meshlets together take more than
    /// [`MOST_MESHLET_WORDS`].
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

    /// The index among the frame's culled runs of the draws' `run`th, if it's culled.
    pub(super) fn of_run(&self, run: usize) -> Option<usize> {
        Some((*self.of_run.get(run)?)? as usize)
    }

    /// The index among the frame's culled runs of the scene's `run`th, if it's culled.
    pub(super) fn of_scene(&self, run: usize) -> Option<usize> {
        Some((*self.of_scene.get(run)?)? as usize)
    }

    /// Whether the `run`th of the frame's culled runs is drawn by its meshlets.
    pub(super) fn by_meshlets(&self, run: usize) -> bool {
        self.runs[run].meshlets != NO_MESHLETS
    }

    /// The addresses of the arrays of the instances `views` views keep, in the graph's buffer at
    /// `buffer`: their transforms, past which `previous` arrays of transforms the frame before
    /// follow, then their IDs and parameters, each array's one view's after another's.
    fn kept(&self, buffer: u64, views: u64, previous: u64) -> [u64; 3] {
        let ids = buffer + (views + previous) * u64::from(self.instances) * TRANSFORM;
        [buffer, ids, ids + views * u64::from(self.instances) * ID]
    }
}

/// How many arrays of transforms the frame before the culling keeps past its views': the game's
/// view's first pass's, then its second's, as [`previous_of`] places them.
const PREVIOUS: u64 = 2;

/// Where the transforms the frame before of what the game's view's pass in slot `slot` keeps are,
/// among `views` views' kept transforms' arrays, by index: past every view's, the first pass's
/// then the second's.
fn previous_of(slot: usize, views: usize) -> u32 {
    (views + usize::from(slot != FIRST)) as u32
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
    /// The words of each view's meshlets, as [`meshlet_words`] counts them for each run drawn by
    /// them, up to [`MOST_MESHLET_WORDS`].
    meshlet_words: u64,
    /// How many runs are drawn by their levels' indices: those not drawn by meshlets, and those
    /// past what [`MOST_MESHLET_WORDS`] holds.
    by_indices: u64,
}

impl Bounds {
    /// Adds a run of `count` instances whose shader takes `params_words` words of parameters for
    /// each, drawn by its mesh's meshlets, `placed`, if it's drawn by them, as far as
    /// [`MOST_MESHLET_WORDS`] holds them, and otherwise by its levels' indices.
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

    /// Each view's room for the instances it keeps: their transforms, IDs and parameters.
    fn kept_bytes(&self) -> u64 {
        self.instances * (TRANSFORM + ID) + self.params_words * 4
    }

    /// Each view's room for its commands: a run's levels' each, at most [`MAX_LEVELS`].
    fn commands_bytes(&self) -> u64 {
        self.runs * MAX_LEVELS as u64 * COMMAND
    }

    /// Where the meshlets of the view in slot `slot`, of `views`, start among the commands, in
    /// words: after every view's commands, each view's after the one before's.
    fn meshlets_at(&self, views: u64, slot: u64) -> u64 {
        views * self.commands_bytes() / 4 + slot * self.meshlet_words
    }

    /// Each view's room for its instances' places, then its blocks' counts of each level.
    fn scratch_bytes(&self) -> u64 {
        self.instances * 4 + self.blocks * MAX_LEVELS as u64 * 4
    }
}

/// The depth pyramid of a view `size` pixels: how many levels it has, and their texels together.
/// Its first level is half the view each way, rounded up, and each after half the one before, down
/// to a texel, as `shaders/pyramid.slang` builds them and `shaders/cull.slang` finds them.
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

/// What a frame's culling takes of its graph: the buffers it writes, of a view's room each for
/// as many views as it culls for, what it reads back, and its passes: the first pass's, for the
/// game's view's first pass and the cascades, which go before those that draw what they keep; and
/// once the view's first pass has drawn, the depth pyramid's and the second pass's, before the
/// view's second.
pub(crate) struct CullPlan {
    views: usize,
    bounds: Bounds,
    /// The instances of the runs it culls, gathered as one view's are kept.
    gathered: BufferId,
    /// The instances each view keeps, as [`Culling::kept`] lays them out.
    instances: BufferId,
    /// Each view's commands, then each view's meshlets: each run's draw of them, and those it
    /// keeps.
    commands: BufferId,
    /// Whether each instance was unhidden, kept from frame to frame, then each view's instances'
    /// places and its blocks' counts.
    scratch: BufferId,
    /// What the game's view drew, as `shaders/cull.slang` counts it.
    counts: BufferId,
    gather: PassId,
    choose: PassId,
    offsets: PassId,
    write: PassId,
    meshlets: PassId,
    /// The second pass's, once [`CullPlan::second_pass`] has declared it.
    second: Option<SecondPass>,
}

/// The depth pyramid of the game's view and the culling's second pass, in a frame's graph.
struct SecondPass {
    /// The view's depth, and its size.
    depth: ImageId,
    size: [u32; 2],
    pyramid: BufferId,
    /// The passes that build the pyramid's levels, in order, as many as it has.
    levels: [Option<PassId>; MOST_PYRAMID_LEVELS],
    choose: PassId,
    offsets: PassId,
    write: PassId,
    meshlets: PassId,
    read: PassId,
}

impl CullPlan {
    /// Declares in `graph` the culling of runs within `bounds` for the game's view's two passes,
    /// and for the shadows' cascades, if `cascades`; its first pass's, for the view's first and the
    /// cascades, on the compute queue, the passes that draw what it keeps come after, and take it
    /// as [`CullPlan::drawn`] has them; its second, [`CullPlan::second_pass`], is declared after the
    /// view's first, on the graphics queue, as it samples the view's depth.
    pub(crate) fn new(graph: &mut Graph, bounds: Bounds, cascades: bool) -> Self {
        let views = if cascades { MAX_VIEWS } else { 2 };
        let count = views as u64;
        // Each instance's transform the frame before, of what's gathered and of what the game's
        // view's passes keep.
        let previous = bounds.instances * TRANSFORM;
        let gathered = graph.buffer("gathered instances", bounds.kept_bytes() + previous);
        let kept = count * bounds.kept_bytes() + PREVIOUS * previous;
        let instances = graph.buffer("culled instances", kept);
        let commands_bytes = bounds.commands_bytes() + bounds.meshlet_words * 4;
        let commands = graph.buffer("culled draws", count * commands_bytes);
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

    /// Declares in `graph` the depth pyramid of the game's view, from `depth`, as its first pass
    /// drew it, `size` pixels, and the second pass of the culling, against it, and what reads back
    /// what the view drew: after the view's first pass, and before its second, which takes what
    /// it keeps as [`CullPlan::drawn`] has it.
    ///
    /// # Panics
    ///
    /// If it's declared already.
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

    /// The slot of the game's view's second pass among the views it culls for, the last.
    pub(crate) fn second(&self) -> usize {
        self.views - 1
    }

    /// `pass`, taking the instances and commands of what the culling keeps, and the meshlets it
    /// keeps of the runs drawn by them, which follow the commands.
    pub(crate) fn drawn<'g>(&self, pass: RenderPass<'g>) -> RenderPass<'g> {
        pass.read_buffer(self.instances).indirect(self.commands)
    }

    /// Where the meshlets of the view in slot `slot` start among the commands, in words.
    fn meshlets_at(&self, slot: usize) -> u64 {
        self.bounds.meshlets_at(self.views as u64, slot as u64)
    }

    /// Whether it culls runs drawn by their meshlets, as a visibility buffer draws them.
    pub(crate) fn by_meshlets(&self) -> bool {
        self.bounds.meshlet_words > 0
    }

    /// Whether it may cull runs drawn by their levels' indices, outside a visibility buffer: those
    /// whose meshes have no meshlets, or aren't drawable yet as it's planned, or whose meshlets the
    /// frame has no room for.
    pub(crate) fn by_indices(&self) -> bool {
        self.bounds.by_indices > 0
    }

    /// `pass`, reading what the culling kept of the instances and of their meshlets, for shading
    /// the game's view from its visibility buffer, as [`MeshPass::shade`] does.
    pub(crate) fn shaded<'g>(&self, pass: ComputePass<'g>) -> ComputePass<'g> {
        pass.read_buffer(self.instances).read_buffer(self.commands)
    }
}

/// What shading the game's view from its visibility buffer reads of the frame's culling: the
/// addresses of its runs, of its commands, and of the transforms and the parameters of what its
/// views kept; how many instances each view keeps room for, and words of their parameters; and,
/// for each of the view's two passes, its slot among the views, where its meshlets start among the
/// commands, in words, and where the transforms the frame before of what it keeps are among the
/// kept transforms' arrays, by index.
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
    /// What `pass`, which [`CullPlan::shaded`] declared, reads of what the culling `plan` declared
    /// kept, as [`Kept`] says: no runs in a frame whose culling ran none, whose visibility buffer
    /// holds nothing.
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
            meshlets: slots.map(|slot| plan.meshlets_at(slot) as u32),
            previous: slots.map(|slot| previous_of(slot, plan.views)),
        }
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

/// The name of the buffer the culling counts what the game's view drew in, which frames read back.
pub(crate) const COUNTS: &str = "culled counts";

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

/// A run, as `Run` in `shaders/runs.slang` has it.
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
    /// Its mesh's buffer's address, its low and high words.
    vertices: [u32; 2],
}

/// What the gather copies of a run, as `Copy` in `shaders/gather.slang` has it.
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

/// What the gather pushes, as `Push` in `shaders/gather.slang` has it.
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
    /// The frame's number, its low and high words, which a record's last change is compared with.
    frame: [u32; 2],
    instances: u32,
    unused: u32,
}

/// A level of detail, as `Level` in `shaders/culling.slang` has it.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuLevel {
    start: u32,
    count: u32,
    error: f32,
    meshlet_first: u32,
    meshlet_count: u32,
}

/// A view, as `View` in `shaders/culling.slang` has it.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuView {
    planes: [Vec4; 6],
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
    /// For the game's view's passes, where the transforms the frame before of what it keeps go, as
    /// [`previous_of`] places them; [`NO_PREVIOUS`] for a cascade's.
    previous: u32,
    unused: [u32; 3],
}

/// What a view that keeps no transforms of the frame before has for [`GpuView::previous`]:
/// `NO_PREVIOUS` in `shaders/culling.slang`.
const NO_PREVIOUS: u32 = u32::MAX;

impl GpuView {
    /// `view`, in slot `slot` of `plan`'s, keeping as `keep` says.
    fn new(view: &CullView, plan: &CullPlan, slot: usize, keep: Keep) -> Self {
        let clip = &view.clip_from_world;
        let lod = LodView::new(clip, view.size);
        let [width, height] = view.size;
        Self {
            planes: planes(clip, view.clamped),
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
            meshlets: plan.meshlets_at(slot) as u32,
            eye: eye(clip),
            previous: match keep {
                Keep::All => NO_PREVIOUS,
                Keep::Shown | Keep::Unhidden => previous_of(slot, plan.views),
            },
            unused: [0; 3],
        }
    }
}

/// Where the view `clip_from_world` takes the world into the clip space of sees from, and 1, under
/// perspective, where clip W is 0 at the camera; or, without it, the way it looks, and 0: the way
/// along which clip X and Y stay put and depth falls, as reversed depth falls away from the camera.
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

/// What the culling of meshlets pushes, as `Push` in `shaders/meshlets.slang` has it.
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
    pyramid: u64,
    run_count: u32,
    block_count: u32,
    instances: u32,
    params_words: u32,
}

/// What a pass that builds a level of the depth pyramid pushes, as `Push` in
/// `shaders/pyramid.slang` has it.
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
    /// The most a frame's culling of `draws` and the scene takes, of their runs whose shaders are
    /// opaque, by the shaders the pass has, whatever their meshes, and the meshlets of those drawn
    /// by them, of the meshes drawable as it's asked, as [`Bounds::add`] counts them; `None` if no
    /// run's shader is opaque.
    pub(crate) fn cull_bounds(&self, draws: &Draws) -> Option<Bounds> {
        let mut bounds = Bounds::default();
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

    /// Culls the runs of `draws` and of the scene whose meshes are drawable and whose shaders are
    /// opaque, as `plan` declared in the frame's graph, for its first pass's views, the game's
    /// view, `seen`, in its first pass, and `cascades`, from their instances as `uploaded` has the
    /// draws' and the scene's table has its own, gathered first, and records its passes, the next
    /// of the frame's. Returns whether it culled any run: the frame's read back counts nothing if
    /// it didn't, as no meshes it may cull are drawable yet. [`MeshPass::cull_second`] culls for
    /// the view's second pass.
    ///
    /// # Panics
    ///
    /// If `cascades` aren't as many as `plan` was declared for, or `draws` aren't what bounded it.
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
        // The scene's first, so that where each of its instances is gathered, which what was
        // unhidden is kept by, doesn't move as the draws change.
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
        // In their slots: the view's first pass, the cascades, then its second.
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
        // Laid out as one view's kept instances are, each run's at its `output`, with each one's
        // transform the frame before.
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
            culling.meshlets_dispatch(&mut pass, &meshlets, plan.views - 1);
            culling.meshlets_pushed = Some(meshlets);
        }
        Ok(true)
    }

    /// Builds the depth pyramid of the game's view from its depth, as its first pass drew it, and
    /// culls the runs [`MeshPass::cull`] culled for the view's second pass against it, as `plan`
    /// declared them in the frame's graph, recording their passes, the next of the frame's, and what
    /// reads back what the view drew.
    ///
    /// # Panics
    ///
    /// If `plan` has no second pass declared.
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

    /// Draws the instances that view `slot` of `plan` kept of the `run`th of the runs it culled,
    /// with `push`'s mesh, a draw for each of its mesh's levels, as the commands written for them
    /// say; or its meshlets they kept, into the visibility buffer if `visibility`.
    pub(super) fn draw_culled(
        &self,
        pass: &mut Pass<'_, '_>,
        mesh: &GpuMesh,
        push: Push,
        (plan, slot): (&CullPlan, usize),
        run: usize,
        visibility: bool,
    ) {
        let culling = &self.culling;
        let index = run as u32;
        let run = &culling.runs[run];
        let [transforms, ids, params] =
            culling.kept(pass.address(plan.instances), plan.views as u64, PREVIOUS);
        // The game's view's passes keep their instances' transforms the frame before, past every
        // view's, as many transforms on as `previous`; a cascade's draws read their own.
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
        if run.meshlets != NO_MESHLETS {
            // Its draw's eight words, then the meshlets it keeps, which the shader reads.
            let draw = (plan.meshlets_at(slot as usize) + u64::from(run.meshlet_draw)) * 4;
            // Which run and which of the game's view's passes, as `Push::draw` says.
            let second = slot as usize == plan.second();
            let push = Push {
                meshlets: pass.address(plan.commands) + draw,
                draw: if visibility {
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

/// Where a culled run's instances are, which the gather copies them from: the frame's draws', from
/// their first and the first word of their parameters; or the scene's, from their first in its
/// order.
#[derive(Clone, Copy)]
enum Source {
    Frame { first: u32, params: u32 },
    Scene { first: u32 },
}

/// The culled runs' instances, the words of their parameters, their commands, and the words of
/// each view's meshlets of those drawn by them, so far.
#[derive(Default)]
struct Totals {
    instances: u32,
    params_words: u32,
    commands: u32,
    meshlet_words: u32,
}

impl Totals {
    /// Takes the words of each view's meshlets the draw of a run of `count` instances of a mesh
    /// whose meshlets are `placed` takes, if they're within the `budget` the frame planned, and
    /// returns where its draw starts among them and how many meshlets it has room for; or takes
    /// nothing past it, as the run is then drawn by its levels' indices.
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

/// A run that may be culled: its mesh and shader, and the faces it draws.
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
    /// Records `passes`, the choosing, the offsets and the writing of a pass of the culling, each
    /// pushing `push`, for the first `views` of the views it points to.
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

    /// Records `pass`, the culling of meshlets of a pass of the culling, pushing `push`, for the
    /// first `views` of the views it points to.
    fn meshlets_dispatch(&self, pass: &mut Compute<'_, '_>, push: &MeshletPush, views: usize) {
        let groups = u64::from(push.block_count).min(MOST_GROUPS) as u32;
        pass.set_pipeline(&self.meshlets);
        pass.push_constants(bytemuck::bytes_of(push));
        pass.dispatch([groups, views as u32, 1]);
    }

    /// Adds `run`, of `count` instances at `source`, to the runs culled, its blocks and their
    /// copies, if its mesh is drawable and its shader opaque, and returns its index among them;
    /// `None` if it isn't culled. A run whose mesh has meshlets and whose shader draws them is
    /// drawn by them, with room for them in each view's meshlets, if the frame planned it.
    ///
    /// # Panics
    ///
    /// If its instances don't fit the 32-bit count Vulkan takes, or the scene's instances take
    /// parameters of more than a [`MeshDraw`] holds.
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
        // Drawn by its meshlets where the frame planned room for them: its draw's start among each
        // view's meshlets, and how many it has room for.
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

    /// A view under perspective sees from its camera, and one without looks the way its camera
    /// faces, whatever it's turned to, as meshlets' cones are tested from.
    #[test]
    fn a_view_sees_from_its_camera_or_looks_its_way() {
        let rotation = Quat::from_euler(gg_core::math::EulerRot::YXZ, 0.7, -0.4, 0.2);
        let position = Vec3::new(3.0, -2.0, 5.5);
        let seen = |projection| {
            let viewpoint = Viewpoint {
                position,
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

    /// A frame plans room in each view for the meshlets of its runs drawn by them up to
    /// [`MOST_MESHLET_WORDS`], however many such runs it has; and its culling draws by their
    /// meshlets the runs whose draws fit what it planned, in turn, and those past it by their
    /// levels' indices, taking no room for them. The plan counts as drawn by their indices the runs
    /// without meshlets, and each that may be, as it comes once the room is full.
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

    /// A depth pyramid halves the view, rounding up, level by level down to a texel, and holds
    /// each level's texels.
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
