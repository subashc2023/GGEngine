// GPU culling of a frame's opaque runs, per view: the game's view's two passes and the shadow
// cascades. An instance is kept if its bounding sphere reaches inside every plane of the view, at
// the level of detail `LodView` in src/mesh.rs picks. Kept instances go into the view's arrays, by
// level in order, for the run's indirect draws, one per level. `Push` matches `CullPush` in
// src/mesh/cull.rs field for field; culling.slang holds what meshlets.slang shares.
//
// The game's view's first pass keeps what its second found unhidden last frame. The second keeps
// what the first didn't and the depth pyramid doesn't hide, and records what's unhidden for the
// next frame: what comes into view draws that frame. A cascade keeps all inside its planes.
// Unhidden is a word per instance in gathered order, kept from frame to frame. Instances coming or
// going earlier in that order shift it: the first pass then draws some hidden and leaves out some
// shown, which the second draws.
//
// Three passes, for the first pass's views, then, once the pyramid is built, the second's; views
// by the group's Y:
// - `choose_main`: each instance's level, and each block's count per level.
// - `offsets_main`: those summed per run into its commands and each block's place; and the draws
//   of runs drawn by meshlets, which meshlets.slang then culls.
// - `write_main`: each kept instance copied to its place.
//
// Each array is an indexed pointer from the push constants, each view's part an index: no pointer
// is held in buffer memory, nor offset and kept in a variable. AMD's driver (25.10.30.02)
// miscompiles both: it loaded a `uint*` read through a held address as its low word twice, and
// carried a pointer offset from a pushed one across a loop as another pushed one.

#include "culling.slang"

// For a meshlet's draw through vertex shaders: three per triangle of a meshlet's most,
// `MAX_MESHLET_TRIANGLES` in src/packed.rs.
static const uint MESHLET_VERTICES = 372;

struct Push {
    Run* runs;
    // Each block's run and first instance in it.
    uint2* blocks;
    Level* levels;
    View* views;
    // As `gather.slang` gathered them, last frame's transforms `instances` past.
    float4x4* transforms;
    uint2* ids;
    uint* params;
    // Each view's kept instances, `instances` and `params_words` long, each run's at its `output`.
    // Past the views' transforms, last frame's of the game's view's, at `View::previous`.
    float4x4* kept_transforms;
    uint2* kept_ids;
    uint* kept_params;
    // Each view's, `run_count * MAX_LEVELS`. Then each view's meshlets at its `meshlets`, in words:
    // per run an eight-word draw, then its kept meshlets, two words each.
    Command* commands;
    // Whether each instance was unhidden, `instances` long, then per view what `codes_at` and
    // `block_levels_at` place.
    uint* scratch;
    // Drawn in the game's view: draws with an instance, instances, then triangles as a 64-bit
    // count, low word first.
    uint* counts;
    // For `KEEP_UNHIDDEN`: the game's view's, as `pyramid.slang` builds it.
    float* pyramid;
    uint run_count;
    uint block_count;
    // The runs' instances and parameter words.
    uint instances;
    uint params_words;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Per level, 8 bits each, four to a word: a level keeps at most the block's 64.
groupshared uint2 sums[BLOCK];

// The level `view` draws the instance at, or `CULLED`. `KEEP_UNHIDDEN` also records whether it's
// unhidden, for the next frame.
uint choose(Run run, View view, float4x4 transform, uint gathered) {
    float3x3 columns = transpose((float3x3)transform);
    float3 x = columns[0];
    float3 y = columns[1];
    float3 z = columns[2];
    float3 lengths = float3(dot(x, x), dot(y, y), dot(z, z));
    float3 center = mul(transform, float4(run.sphere.xyz, 1.0)).xyz;
    bool inside = true;
    bool hidden = false;
    if (!isinf(run.sphere.w)) {
        // Grown by the transform's most stretch.
        float radius = run.sphere.w * stretch(x, y, z);
        inside = reaches(view, center, radius);
        if (inside && view.keep == KEEP_UNHIDDEN) {
            hidden = hides(view, center, radius);
        }
    }
    if (view.keep == KEEP_UNHIDDEN) {
        push.scratch[gathered] = inside && !hidden ? 1 : 0;
    }
    if (!inside) {
        return CULLED;
    }
    if (view.keep == KEEP_SHOWN && push.scratch[gathered] == 0) {
        return CULLED;
    }
    if (view.keep == KEEP_UNHIDDEN
        && (hidden || push.scratch[codes_at(FIRST) + gathered] != CULLED)) {
        return CULLED;
    }
    if (run.level_count == 1) {
        return 0;
    }
    // As `LodView::level`: the coarsest level whose scaled error spans at most a pixel at the
    // sphere's nearest; the finest if it reaches the camera's plane, or on NaN.
    float scale = sqrt(max(lengths.x, max(lengths.y, lengths.z)));
    float w = dot(view.w.xyz, center) + view.w.w;
    float near = w - run.sphere.w * scale * view.w_slope;
    if (!(near > 0.0)) {
        return 0;
    }
    float allowed = LOD_ERROR_PIXELS * near / (scale * view.pixels_per_unit);
    uint level = 0;
    for (uint i = 0; i < run.level_count; i++) {
        if (push.levels[run.levels + i].error <= allowed) {
            level = i;
        }
    }
    return level;
}

// Each instance's code, as `codes_at` says, and the block's count per level.
[shader("compute")]
[numthreads(BLOCK, 1, 1)]
void choose_main(uint3 group: SV_GroupID, uint3 thread: SV_GroupThreadID) {
    View view = push.views[group.y];
    uint codes = codes_at(view.slot);
    uint block_levels = block_levels_at(view.slot);
    if (view.slot == FIRST && group.x == 0 && thread.x < 4) {
        // `offsets_main` counts into these in both passes.
        push.counts[thread.x] = 0;
    }
    // Uniform across the group, so every thread reaches each barrier.
    for (uint block = group.x; block < push.block_count; block += groups()) {
        uint2 at = push.blocks[block];
        Run run = push.runs[at.x];
        uint i = at.y + thread.x;
        uint level = CULLED;
        if (i < run.count) {
            level = choose(run, view, push.transforms[run.first + i], run.output + i);
        }
        uint2 one = uint2(0, 0);
        if (level != CULLED) {
            uint bit = 1u << ((level & 3) * 8);
            one = level < 4 ? uint2(bit, 0) : uint2(0, bit);
        }
        // An inclusive Hillis and Steele scan.
        sums[thread.x] = one;
        GroupMemoryBarrierWithGroupSync();
        for (uint offset = 1; offset < BLOCK; offset <<= 1) {
            uint2 before = thread.x >= offset ? sums[thread.x - offset] : uint2(0, 0);
            GroupMemoryBarrierWithGroupSync();
            sums[thread.x] += before;
            GroupMemoryBarrierWithGroupSync();
        }
        if (i < run.count) {
            uint code = CULLED;
            if (level != CULLED) {
                uint2 earlier = sums[thread.x] - one;
                uint word = level < 4 ? earlier.x : earlier.y;
                code = level | ((word >> ((level & 3) * 8)) & 0xFF) << 8;
            }
            push.scratch[codes + run.output + i] = code;
        }
        if (thread.x < run.level_count) {
            uint2 total = sums[BLOCK - 1];
            uint word = thread.x < 4 ? total.x : total.y;
            push.scratch[block_levels + block * MAX_LEVELS + thread.x] =
                (word >> ((thread.x & 3) * 8)) & 0xFF;
        }
        // Before the next block's scan overwrites this one's.
        GroupMemoryBarrierWithGroupSync();
    }
}

// A thread a run; each level's place in the run's room follows the last's.
[shader("compute")]
[numthreads(BLOCK, 1, 1)]
void offsets_main(uint3 group: SV_GroupID, uint3 thread: SV_GroupThreadID) {
    uint r = group.x * BLOCK + thread.x;
    if (r >= push.run_count) {
        return;
    }
    View view = push.views[group.y];
    // Only the game's view's passes count.
    bool counted = view.keep != KEEP_ALL;
    uint block_levels = block_levels_at(view.slot);
    uint commands = view.slot * push.run_count * MAX_LEVELS;
    Run run = push.runs[r];
    uint totals[MAX_LEVELS];
    for (uint l = 0; l < MAX_LEVELS; l++) {
        totals[l] = 0;
    }
    for (uint b = run.blocks; b < run.blocks + run.block_count; b++) {
        for (uint l = 0; l < run.level_count; l++) {
            totals[l] += push.scratch[block_levels + b * MAX_LEVELS + l];
        }
    }
    uint starts[MAX_LEVELS];
    uint start = 0;
    uint draws = 0;
    uint instances = 0;
    // `meshlets_main` counts the triangles of runs drawn by meshlets.
    bool by_meshlets = run.meshlets != NO_MESHLETS;
    for (uint l = 0; l < run.level_count; l++) {
        starts[l] = start;
        start += totals[l];
        Level level = push.levels[run.levels + l];
        Command command = { level.count, totals[l], level.start, 0, starts[l] };
        push.commands[commands + run.commands + l] = command;
        if (counted && totals[l] > 0) {
            draws += 1;
            instances += totals[l];
            if (!by_meshlets) {
                add_wide(2, totals[l] * (level.count / 3));
            }
        }
    }
    if (by_meshlets) {
        // Its meshlet draw, which `meshlets_main` fills: `MESHLET_VERTICES` per kept meshlet
        // through vertex shaders, or a workgroup each, `ACROSS` across, through a mesh shader;
        // then the count drawn, and the count kept, which may pass its room.
        uint record = view.meshlets + run.meshlet_draw;
        ((uint*)push.commands)[record] = MESHLET_VERTICES;
        ((uint*)push.commands)[record + 1] = 0;
        ((uint*)push.commands)[record + 2] = 0;
        ((uint*)push.commands)[record + 3] = 0;
        ((uint*)push.commands)[record + 4] = 0;
        ((uint*)push.commands)[record + 5] = 0;
        ((uint*)push.commands)[record + 6] = 1;
        ((uint*)push.commands)[record + 7] = 0;
    }
    for (uint b = run.blocks; b < run.blocks + run.block_count; b++) {
        for (uint l = 0; l < run.level_count; l++) {
            uint at = block_levels + b * MAX_LEVELS + l;
            uint kept = push.scratch[at];
            push.scratch[at] = starts[l];
            starts[l] += kept;
        }
    }
    if (counted) {
        InterlockedAdd(push.counts[0], draws);
        InterlockedAdd(push.counts[1], instances);
    }
}

[shader("compute")]
[numthreads(BLOCK, 1, 1)]
void write_main(uint3 group: SV_GroupID, uint3 thread: SV_GroupThreadID) {
    View view = push.views[group.y];
    uint codes = codes_at(view.slot);
    uint block_levels = block_levels_at(view.slot);
    uint kept = view.slot * push.instances;
    uint kept_params = view.slot * push.params_words;
    for (uint block = group.x; block < push.block_count; block += groups()) {
        uint2 at = push.blocks[block];
        Run run = push.runs[at.x];
        uint i = at.y + thread.x;
        if (i >= run.count) {
            continue;
        }
        uint code = push.scratch[codes + run.output + i];
        if (code == CULLED) {
            continue;
        }
        uint place = push.scratch[block_levels + block * MAX_LEVELS + (code & 0xFF)] + (code >> 8);
        // Within the run's room, whatever `offsets_main` placed: past it, a write could leave the
        // view's arrays.
        if (place >= run.count) {
            continue;
        }
        uint from = run.first + i;
        push.kept_transforms[kept + run.output + place] = push.transforms[from];
        if (view.previous != NO_PREVIOUS) {
            push.kept_transforms[view.previous * push.instances + run.output + place] =
                push.transforms[push.instances + from];
        }
        push.kept_ids[kept + run.output + place] = push.ids[from];
        for (uint w = 0; w < run.params_words; w++) {
            push.kept_params[kept_params + run.params_output + place * run.params_words + w] =
                push.params[run.params + i * run.params_words + w];
        }
    }
}
