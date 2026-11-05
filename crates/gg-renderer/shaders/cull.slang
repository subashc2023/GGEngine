// Culls the instances of a frame's opaque runs on the GPU, for each view that draws them: the
// game's view, in each of its two passes, and the shadows' cascades. Each instance is kept if the
// sphere round its mesh's vertices, as its transform places it, reaches inside every plane of the
// view, and draws at the level of detail the render thread would pick for it, as `LodView` in
// src/mesh.rs picks; those kept are written, each level's in the order they came, into the view's
// arrays, where the run's indirect draws, one for each of its mesh's levels, take them, as the
// commands written for them say. `Run`, `Level`, `View` and `Push` match `GpuRun`, `GpuLevel`,
// `GpuView` and `CullPush` in src/mesh/cull.rs field for field.
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
// those up across a run's blocks, into its commands and where each block's go; and `write_main`,
// which copies each instance kept to where it goes. They run for the first pass's views, the game's
// view's first and the cascades, and then, once the first pass has drawn and the pyramid is built,
// again for the second's.
//
// Each array is reached as a pointer the push constants hold, indexed, each view's part of it found
// by an index too: no pointer is held in buffer memory, nor offset and kept in a variable. AMD's
// driver (25.10.30.02) miscompiles both: it loaded only the low word of a `uint*` that a struct read
// through an address held, and took it for the high word too; and it carried a pointer offset from
// one pushed across a loop as another pushed one.

// How many of a run's instances a workgroup takes: `BLOCK` in src/mesh/cull.rs.
static const uint BLOCK = 64;
// The most workgroups a dispatch takes across: `MOST_GROUPS` in src/mesh/cull.rs.
static const uint MOST_GROUPS = 65535;
// The most levels of detail a mesh has: `MAX_LEVELS` in src/mesh.rs.
static const uint MAX_LEVELS = 8;
// Where an instance a view doesn't draw goes: nowhere.
static const uint CULLED = 0xFFFFFFFF;
// How many pixels a level's error may span on screen: `LOD_ERROR_PIXELS` in src/mesh.rs.
static const float LOD_ERROR_PIXELS = 1.0;
// The slot of the game's view's first pass among the views: `FIRST` in src/mesh/cull.rs.
static const uint FIRST = 0;

// What a view keeps of what reaches inside its planes, as `Keep` in src/mesh/cull.rs has it: all of
// it, as a cascade does; what the last frame found unhidden, as the game's view's first pass does;
// or what its first pass didn't keep and the depth pyramid doesn't hide, as its second does.
static const uint KEEP_ALL = 0;
static const uint KEEP_SHOWN = 1;
static const uint KEEP_UNHIDDEN = 2;

// A run of a mesh's instances, drawn with one shader.
struct Run {
    // Its first instance among the frame's, and how many it has.
    uint first;
    uint count;
    // The first word of its instances' parameters among the frame's, and how many each takes.
    uint params;
    uint params_words;
    // The sphere round its mesh's vertices, in the mesh's own space: its center and radius, which
    // is infinite for a mesh whose vertices the renderer can't read, which is never culled.
    float4 sphere;
    // Its mesh's levels' first in the frame's table, and how many there are.
    uint levels;
    uint level_count;
    // Its first instance in each view's arrays, which hold room for all of its instances, and the
    // first word of its parameters there.
    uint output;
    uint params_output;
    // Its first command in each view's, one for each level.
    uint commands;
    // Its first block among the frame's, and how many it has.
    uint blocks;
    uint block_count;
};

// A level of detail: its indices, and how far its surface strays from the mesh's own.
struct Level {
    uint start;
    uint count;
    float error;
};

// A `VkDrawIndexedIndirectCommand`.
struct Command {
    uint index_count;
    uint instance_count;
    uint first_index;
    int vertex_offset;
    uint first_instance;
};

// A view that draws the runs.
struct View {
    // A point `p` is inside a plane where `dot(xyz, p) + w` is at least 0, and a sphere reaches
    // inside it where that's at least minus its radius times the length of `xyz`.
    float4 planes[6];
    // As `LodView`: what a point's clip W is the dot product of, how much W changes with a world
    // unit's distance, and the pixels a world unit spans where W is 1.
    float4 w;
    // What a point's clip X, Y and Z are the dot products of, which the pyramid's test projects
    // spheres by.
    float4 x;
    float4 y;
    float4 z;
    float w_slope;
    float pixels_per_unit;
    // Its place among the views, which its parts of the arrays, the scratch and the commands are
    // at, and what it keeps, one of `KEEP_ALL`, `KEEP_SHOWN` and `KEEP_UNHIDDEN`.
    uint slot;
    uint keep;
    // For `KEEP_UNHIDDEN`, its size in pixels, and how many levels its pyramid has.
    uint width;
    uint height;
    uint pyramid_levels;
    uint unused;
};

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
    // Each view's commands, `run_count` times `MAX_LEVELS` long.
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

// Where the codes of the view in slot `slot` start in `push.scratch`: each instance's level, and
// its place among those of its level in its block, as `level | place << 8`, or `CULLED`, at its
// run's `output`.
uint codes_at(uint slot) {
    return push.instances + slot * (push.instances + push.block_count * MAX_LEVELS);
}

// Where the blocks' levels of the view in slot `slot` start in `push.scratch`: for each block, by
// its index times `MAX_LEVELS`, how many of each level the view keeps of it, as `choose_main` counts
// them, and then where the first of them goes in its run's room, as `offsets_main` places them.
uint block_levels_at(uint slot) {
    return codes_at(slot) + push.instances;
}

// How many workgroups were dispatched across, each taking every that many blocks in turn.
uint groups() {
    return min(push.block_count, MOST_GROUPS);
}

// A block's counts of each level, 8 bits a level, four levels to a word: no level of a block keeps
// more than its 64 instances.
groupshared uint2 sums[BLOCK];

// The least and the most of a clip coordinate over W across a sphere of `radius` round a center
// whose coordinate is `c`, of a row whose XYZ is `length` long, and whose W is `w`, `w_slope` as
// the view has it. Without perspective, W is 1. With it, W is the distance ahead of the camera, and
// the row a scale times the axis it's along, at right angles to the way ahead: the sphere is a disc
// in the plane of the two, `c / length` along the axis and `w` ahead, whose tangents through the
// camera bound it (Mara and McGuire, "2D Polyhedral Bounds of a Clipped, Perspective-Projected 3D
// Sphere", 2013), for a sphere wholly ahead of the camera.
float2 across(float c, float length, float w, float radius, float w_slope) {
    if (w_slope == 0.0) {
        return float2(c - radius * length, c + radius * length);
    }
    float a = c / length;
    float tangent = sqrt(max(a * a + w * w - radius * radius, 0.0));
    float least = (a * tangent - radius * w) / (w * tangent + radius * a);
    float most = (a * tangent + radius * w) / (w * tangent - radius * a);
    return float2(least, most) * length;
}

// Whether `view`'s depth pyramid hides the sphere round `center` of `radius`, in the world: whether
// what's drawn in every pixel the sphere may cover is nearer than its nearest point. Its rectangle
// on screen, a pixel wider each way against rounding, falls within 2 by 2 texels of the level whose
// texels are at least as wide; the farthest of those is as far as anything there is drawn.
bool hides(View view, float3 center, float radius) {
    float4 p = float4(center, 1.0);
    float w = dot(view.w, p);
    // The W of its nearest point: one that reaches the camera's plane has no rectangle.
    float near = w - radius * view.w_slope;
    if (!(near > 0.0)) {
        return false;
    }
    float3 lengths = float3(length(view.x.xyz), length(view.y.xyz), length(view.z.xyz));
    float2 xs = across(dot(view.x, p), lengths.x, w, radius, view.w_slope);
    float2 ys = across(dot(view.y, p), lengths.y, w, radius, view.w_slope);
    // As near as it comes: no Z over the sphere is more, nor W less.
    float depth = (dot(view.z, p) + radius * lengths.z) / near;
    float2 size = float2(view.width, view.height);
    float2 least = (float2(xs.x, ys.x) * 0.5 + 0.5) * size - 1.0;
    float2 most = (float2(xs.y, ys.y) * 0.5 + 0.5) * size + 1.0;
    if (any(isnan(least)) || any(isnan(most))) {
        return false;
    }
    uint2 first = uint2(clamp(least, float2(0.0), size - 1.0));
    uint2 last = uint2(clamp(most, float2(0.0), size - 1.0));
    // The least level whose texels, 2 to the level plus 1 pixels a side, are as wide as the
    // rectangle, which then reaches 2 at most each way.
    uint span = max(last.x - first.x, last.y - first.y) + 1;
    uint level = span <= 1 ? 0 : firstbithigh(span - 1);
    level = min(level, view.pyramid_levels - 1);
    uint at = 0;
    uint2 texels = (uint2(view.width, view.height) + 1) / 2;
    for (uint i = 0; i < level; i++) {
        at += texels.x * texels.y;
        texels = (texels + 1) / 2;
    }
    uint2 a = first >> (level + 1);
    uint2 b = last >> (level + 1);
    float farthest = min(
        min(push.pyramid[at + a.y * texels.x + a.x], push.pyramid[at + a.y * texels.x + b.x]),
        min(push.pyramid[at + b.y * texels.x + a.x], push.pyramid[at + b.y * texels.x + b.x]));
    return depth < farthest;
}

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
        // The sphere grown by the most the transform stretches any way: the square root of the
        // largest eigenvalue of its columns' dot products, which Gershgorin's circles bound by the
        // largest sum of a row's magnitudes, exactly where the columns are at right angles, as a
        // rotation and scale's are, and above it for a shear.
        float xy = abs(dot(x, y));
        float xz = abs(dot(x, z));
        float yz = abs(dot(y, z));
        float stretch = sqrt(max(lengths.x + xy + xz, max(lengths.y + xy + yz, lengths.z + xz + yz)));
        float radius = run.sphere.w * stretch;
        for (uint i = 0; i < 6; i++) {
            float4 plane = view.planes[i];
            if (dot(plane.xyz, center) + plane.w < -radius * length(plane.xyz)) {
                inside = false;
            }
        }
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

// Adds `value` to the 64-bit count whose low word is at `low` in `push.counts` and high word after
// it.
void add_wide(uint low, uint value) {
    uint was;
    InterlockedAdd(push.counts[low], value, was);
    if (was + value < was) {
        InterlockedAdd(push.counts[low + 1], 1);
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
    for (uint l = 0; l < run.level_count; l++) {
        starts[l] = start;
        start += totals[l];
        Level level = push.levels[run.levels + l];
        Command command = { level.count, totals[l], level.start, 0, starts[l] };
        push.commands[commands + run.commands + l] = command;
        if (counted && totals[l] > 0) {
            draws += 1;
            instances += totals[l];
            add_wide(2, totals[l] * (level.count / 3));
        }
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
