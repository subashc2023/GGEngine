// Culls the instances of a frame's opaque runs on the GPU, for each view that draws them: the
// game's view, in each of its two passes, and the shadows' cascades. Each instance is kept if the
// sphere round its mesh's vertices, as its transform places it, reaches inside every plane of the
// view, and draws at the level of detail the render thread would pick for it, as `LodView` in
// src/mesh.rs picks; those kept are written, each level's in the order they came, into the view's
// arrays, where the run's indirect draws, one for each of its mesh's levels, take them, as the
// commands written for them say. `Push` matches `CullPush` in src/mesh/cull.rs field for field, and
// culling.slang has what it shares with meshlets.slang.
//
// The game's view keeps, in its first pass, what its second found unhidden the frame before; and in
// its second, what its first didn't keep that the depth pyramid built from the first's depth doesn't
// hide, finding again what's unhidden for the next frame's first, so that what comes into view
// draws in the frame it does. A cascade keeps all that reaches inside its planes. What's unhidden
// is a word for each instance, where it's gathered, the scene's before the draws', at the start of
// the scratch, which the culling's buffer keeps from frame to frame: a frame in which instances
// before one in that order came or went reads, for that one, what was another's, so that its first
// pass draws some that are hidden and leaves out some that show, which its second then draws.
//
// It runs as three passes, each waiting for the one before: `choose_main`, which finds where each
// instance draws, and how many of each level each block of a run keeps; `offsets_main`, which adds
// those up across a run's blocks, into its commands and where each block's go, and sets out the
// draw of a run drawn by its meshlets; and `write_main`, which copies each instance kept to where
// it goes, after which meshlets.slang culls the meshlets of those drawn by them. They run for the
// first pass's views, the game's view's first and the cascades, and then, once the first pass has
// drawn and the pyramid is built, again for the second's.
//
// Each array is reached as a pointer the push constants hold, indexed, each view's part of it found
// by an index too: no pointer is held in buffer memory, nor offset and kept in a variable. AMD's
// driver (25.10.30.02) miscompiles both: it loaded only the low word of a `uint*` that a struct read
// through an address held, and took it for the high word too; and it carried a pointer offset from
// one pushed across a loop as another pushed one.

#include "culling.slang"

// How many vertices a meshlet's draw through vertex shaders takes, three for each of the most
// triangles a meshlet has, `MAX_MESHLET_TRIANGLES` in src/packed.rs.
static const uint MESHLET_VERTICES = 372;

struct Push {
    Run* runs;
    // Each block's run, and its first instance in the run.
    uint2* blocks;
    Level* levels;
    // The views this dispatch culls for, by the group's Y.
    View* views;
    // The frame's instances, of every run, as `gather.slang` gathered them.
    float4x4* transforms;
    uint2* ids;
    uint* params;
    // The arrays of the instances each view keeps, `instances` and `params_words` long, one view's
    // after another's, each run's at its `output`.
    float4x4* kept_transforms;
    uint2* kept_ids;
    uint* kept_params;
    // Each view's commands, `run_count` times `MAX_LEVELS` long, and after them all, each view's
    // meshlets, where its `meshlets` says, in words: each run's draw of its meshlets, eight words,
    // then its meshlets kept, two words each, as meshlets.slang keeps them.
    Command* commands;
    // Whether each instance was unhidden, `instances` long, then each view's codes and its blocks'
    // levels, as `codes_at` and `block_levels_at` place them.
    uint* scratch;
    // What the game's view drew: its draws with an instance, its instances, and their triangles,
    // as the low and the high word of a 64-bit count.
    uint* counts;
    // The game's view's depth pyramid, as `pyramid.slang` builds it, for `KEEP_UNHIDDEN`.
    float* pyramid;
    uint run_count;
    uint block_count;
    // The runs' instances, and the words of their parameters.
    uint instances;
    uint params_words;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// A block's counts of each level, 8 bits a level, four levels to a word: no level of a block keeps
// more than its 64 instances.
groupshared uint2 sums[BLOCK];

// Where the `gathered`th instance, of `run`, that `transform` places in the world draws in `view`:
// the level it draws at, or `CULLED`. For `KEEP_UNHIDDEN`, it writes too whether the instance is
// unhidden, for the next frame's first pass.
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
        // The sphere grown by the most the transform stretches any way.
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
    // As `LodView::level`: the coarsest level whose error, scaled as the mesh is, spans no more
    // than a pixel where the sphere is nearest, or the finest where the sphere reaches the
    // camera's plane, or anything isn't a number.
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

// For each block of each view, the group's Y: where each of its instances draws, and its place
// among those of its level in the block, and how many of each level it keeps.
[shader("compute")]
[numthreads(BLOCK, 1, 1)]
void choose_main(uint3 group: SV_GroupID, uint3 thread: SV_GroupThreadID) {
    View view = push.views[group.y];
    uint codes = codes_at(view.slot);
    uint block_levels = block_levels_at(view.slot);
    if (view.slot == FIRST && group.x == 0 && thread.x < 4) {
        // `offsets_main` counts into these, after this pass and the second's.
        push.counts[thread.x] = 0;
    }
    // The same for every thread of the group, so that each reaches each barrier.
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
        // An inclusive scan of the block's counts, by Hillis and Steele's steps.
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
        // Before the next block's scan writes over this one's.
        GroupMemoryBarrierWithGroupSync();
    }
}

// For each run of each view, the group's Y: how many of each level its blocks keep, its commands
// and where each block's go, from the first of its room, each level's after the level before's.
[shader("compute")]
[numthreads(BLOCK, 1, 1)]
void offsets_main(uint3 group: SV_GroupID, uint3 thread: SV_GroupThreadID) {
    uint r = group.x * BLOCK + thread.x;
    if (r >= push.run_count) {
        return;
    }
    View view = push.views[group.y];
    // The game's view's two passes count what they draw.
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
    // A run drawn by its meshlets counts its triangles as `meshlets_main` keeps them.
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
        // Its draw of its meshlets, which `meshlets_main` fills: through vertex shaders,
        // `MESHLET_VERTICES` vertices for each meshlet kept, and through a mesh shader, a workgroup
        // for each, `ACROSS` across, as many rows down as they take; how many are drawn, and how
        // many were kept, which may be more, past its room.
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

// For each block of each view, the group's Y: each instance kept, copied to where it goes.
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
        // Within the run's room, as `offsets_main` places them, which this keeps to whatever it
        // placed: a write past it could reach past the view's arrays.
        if (place >= run.count) {
            continue;
        }
        uint from = run.first + i;
        push.kept_transforms[kept + run.output + place] = push.transforms[from];
        push.kept_ids[kept + run.output + place] = push.ids[from];
        for (uint w = 0; w < run.params_words; w++) {
            push.kept_params[kept_params + run.params_output + place * run.params_words + w] =
                push.params[run.params + i * run.params_words + w];
        }
    }
}
