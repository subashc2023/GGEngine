// Culls the meshlets of the instances cull.slang kept, of runs drawn by meshlets, for each view
// that draws them. A meshlet of its instance's level is kept where:
// - its sphere, placed by the instance's transform, reaches inside every plane of the view;
// - for a run drawing fronts alone, of an instance whose transform keeps angles, the view sees its
//   triangles' fronts, by their cone;
// - in the game's view's second pass, the depth pyramid doesn't hide it. The first pass runs
//   before the pyramid is built.
// Each is written into its run's draw in the view, as its instance's place among the run's kept
// instances and its index in its mesh, for model.slang's `meshlet_vertex_main` and `mesh_main`.
// The game's view counts their triangles.
//
// A workgroup takes blocks of a run's instances in turn, as cull.slang's passes do, and their
// meshlets a thread each. Each round's kept meshlets are placed by a group count, then by one
// atomic add to the run's draw. Every array is reached through a push-constant pointer, indexed, as
// in cull.slang. `Push` matches `MeshletPush` in src/mesh/cull.rs field for field.

#include "culling.slang"

// Workgroups across a mesh shader's draw of meshlets, rows as needed: `MESHLET_ACROSS` in
// src/mesh/cull.rs, `ACROSS` in model.slang.
static const uint ACROSS = 32768;

// How near a transform's axes must be to one length and right angles, as a fraction of the
// longest's square, for a meshlet's cone to turn with it: a rotation, mirror or uniform scale.
static const float SIMILAR = 1e-3;

// `MeshletBounds` in src/packed.rs, in the renderer's table.
struct MeshletBounds {
    float3 center;
    float radius;
    // Its cone's axis's X, Y and Z, then its cutoff, a signed byte each, X lowest.
    uint cone;
    uint triangles;
};

struct Push {
    Run* runs;
    // Each block's run, and its first instance in the run.
    uint2* blocks;
    Level* levels;
    // By the group's Y.
    View* views;
    // The frame's instances' transforms, as gather.slang gathered them.
    float4x4* transforms;
    // The scratch, as cull.slang left it: each view's codes and its blocks' levels.
    uint* scratch;
    // What the game's view drew, as cull.slang counts it.
    uint* counts;
    // The game's view's depth pyramid, for `KEEP_UNHIDDEN`.
    float* pyramid;
    // The renderer's table of meshlets' bounds, each run's mesh's from its `meshlets`.
    MeshletBounds* bounds;
    // The culling's commands, as words: each view's meshlets where its `meshlets` says.
    uint* commands;
    uint run_count;
    uint block_count;
    uint instances;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// A round's kept meshlets, their triangles, and where the first goes in the run's draw.
groupshared uint round_kept;
groupshared uint round_triangles;
groupshared uint round_first;

// Whether `view` sees only the backs of a meshlet's triangles, by its sphere in the world and its
// `cone` in mesh space, of an instance whose `transform` keeps angles: meshoptimizer's sphere and
// cone test without the apex (`meshopt_computeClusterBounds`). A cone of no axis faces every way.
bool faces_away(View view, float4x4 transform, float3 center, float radius, uint cone) {
    float3 axis = float3(int(cone << 24) >> 24, int(cone << 16) >> 24, int(cone << 8) >> 24);
    float cutoff = float(int(cone) >> 24) / 127.0;
    float3 turned = mul((float3x3)transform, axis / 127.0);
    float turned_length = length(turned);
    if (!(turned_length > 0.0)) {
        return false;
    }
    turned /= turned_length;
    if (view.eye.w != 0.0) {
        float3 d = center - view.eye.xyz;
        return dot(d, turned) >= cutoff * length(d) + radius;
    }
    return dot(view.eye.xyz, turned) >= cutoff;
}

// For each block of each view (the group's Y): the kept instances' meshlets culled, and those kept
// written into the run's draw.
[shader("compute")]
[numthreads(BLOCK, 1, 1)]
void meshlets_main(uint3 group: SV_GroupID, uint3 thread: SV_GroupThreadID) {
    View view = push.views[group.y];
    uint codes = codes_at(view.slot);
    uint block_levels = block_levels_at(view.slot);
    bool counted = view.keep != KEEP_ALL;
    // Uniform across the group, so every thread reaches each barrier.
    for (uint block = group.x; block < push.block_count; block += groups()) {
        uint2 at = push.blocks[block];
        Run run = push.runs[at.x];
        if (run.meshlets == NO_MESHLETS) {
            continue;
        }
        uint record = view.meshlets + run.meshlet_draw;
        uint end = min(at.y + BLOCK, run.count);
        for (uint i = at.y; i < end; i++) {
            uint code = push.scratch[codes + run.output + i];
            if (code == CULLED) {
                continue;
            }
            uint l = code & 0xFF;
            uint place = push.scratch[block_levels + block * MAX_LEVELS + l] + (code >> 8);
            // Within the run's room, as write_main keeps it.
            if (place >= run.count) {
                continue;
            }
            Level level = push.levels[run.levels + l];
            float4x4 transform = push.transforms[run.first + i];
            float3x3 columns = transpose((float3x3)transform);
            float3 x = columns[0];
            float3 y = columns[1];
            float3 z = columns[2];
            float grown = stretch(x, y, z);
            float3 lengths = float3(dot(x, x), dot(y, y), dot(z, z));
            float longest = max(lengths.x, max(lengths.y, lengths.z));
            float shortest = min(lengths.x, min(lengths.y, lengths.z));
            float skew = max(abs(dot(x, y)), max(abs(dot(x, z)), abs(dot(y, z))));
            bool cones = run.cones != 0 && shortest >= longest * (1.0 - SIMILAR)
                && skew <= longest * SIMILAR;
            for (uint first = 0; first < level.meshlet_count; first += BLOCK) {
                uint m = first + thread.x;
                bool keep = false;
                uint triangles = 0;
                if (m < level.meshlet_count) {
                    MeshletBounds bounds = push.bounds[run.meshlets + level.meshlet_first + m];
                    float3 center = mul(transform, float4(bounds.center, 1.0)).xyz;
                    float radius = bounds.radius * grown;
                    keep = reaches(view, center, radius);
                    if (keep && cones) {
                        keep = !faces_away(view, transform, center, radius, bounds.cone);
                    }
                    if (keep && view.keep == KEEP_UNHIDDEN) {
                        keep = !hides(view, center, radius);
                    }
                    triangles = bounds.triangles;
                }
                if (thread.x == 0) {
                    round_kept = 0;
                    round_triangles = 0;
                }
                GroupMemoryBarrierWithGroupSync();
                uint slot = 0;
                if (keep) {
                    InterlockedAdd(round_kept, 1, slot);
                    InterlockedAdd(round_triangles, triangles);
                }
                GroupMemoryBarrierWithGroupSync();
                if (thread.x == 0 && round_kept > 0) {
                    // Those past the draw's room are dropped; the drawn count still takes them
                    // in, as a diagnostic.
                    uint kept;
                    InterlockedAdd(((uint*)push.commands)[record + 7], round_kept, kept);
                    uint drawn = min(kept + round_kept, run.meshlet_room);
                    InterlockedMax(((uint*)push.commands)[record + 1], drawn);
                    InterlockedMax(((uint*)push.commands)[record + 4], min(drawn, ACROSS));
                    InterlockedMax(((uint*)push.commands)[record + 5], (drawn + ACROSS - 1) / ACROSS);
                    round_first = kept;
                    if (counted) {
                        add_wide(2, round_triangles);
                    }
                }
                GroupMemoryBarrierWithGroupSync();
                if (keep && round_first + slot < run.meshlet_room) {
                    uint entry = record + 8 + 2 * (round_first + slot);
                    ((uint*)push.commands)[entry] = place;
                    ((uint*)push.commands)[entry + 1] = level.meshlet_first + m;
                }
                // Before the next round's count starts again.
                GroupMemoryBarrierWithGroupSync();
            }
        }
    }
}
