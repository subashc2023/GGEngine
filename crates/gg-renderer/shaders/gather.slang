// Gathers the instances of a frame's opaque runs into the arrays `cull.slang` culls them from, each
// run's after the run's before: those of the frame's draws from where the frame copied them, and
// those of the scene, which the GPU keeps from frame to frame, from its table of records, in the
// order the scene keeps them in. `Copy`, `Record` and `Push` match `GpuCopy`, `Record` in
// src/mesh/scene.rs and `GatherPush` in src/mesh/cull.rs field for field.
//
// Each array is reached as a pointer the push constants hold, indexed, as `cull.slang` says why.

// How many of a run's instances a copy takes, a workgroup's: `BLOCK` in src/mesh/cull.rs.
static const uint BLOCK = 64;
// The words of a record's parameters: `MeshDraw::MAX_PARAMS` in src/mesh.rs, in words.
static const uint RECORD_WORDS = 16;
// A copy's `source_params` for one from the scene.
static const uint SCENE = 0xFFFFFFFF;

// Up to `BLOCK` instances of a run, a workgroup's to copy.
struct Copy {
    // Where the first goes in the arrays, and the first word of its parameters there.
    uint dest;
    uint dest_params;
    // How many there are, and the words of each one's parameters.
    uint count;
    uint params_words;
    // Where the first is: in the frame's arrays, its parameters' first word at `source_params`;
    // or, where that's `SCENE`, in the scene's order, which holds each one's slot in its table.
    uint source;
    uint source_params;
};

// An instance of the scene, in its slot of the table.
struct Record {
    float4x4 transform;
    uint2 id;
    uint2 unused;
    uint params[RECORD_WORDS];
};

struct Push {
    Copy* copies;
    // The frame's instances, as it copied them.
    float4x4* frame_transforms;
    uint2* frame_ids;
    uint* frame_params;
    // The scene's table of records, by slot, and its order.
    Record* scene;
    uint* order;
    // Where they go: what `cull.slang` reads.
    float4x4* transforms;
    uint2* ids;
    uint* params;
    uint copy_count;
    // How many workgroups were dispatched across, each taking every that many copies in turn.
    uint groups;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("compute")]
[numthreads(BLOCK, 1, 1)]
void gather_main(uint3 group: SV_GroupID, uint3 thread: SV_GroupThreadID) {
    for (uint c = group.x; c < push.copy_count; c += push.groups) {
        Copy copy = push.copies[c];
        uint i = thread.x;
        if (i >= copy.count) {
            continue;
        }
        uint to = copy.dest + i;
        uint to_params = copy.dest_params + i * copy.params_words;
        if (copy.source_params == SCENE) {
            // Read whole, as the model shaders read the frame's data, so that every address is
            // a pushed one indexed, none kept across the loop.
            Record record = push.scene[push.order[copy.source + i]];
            push.transforms[to] = record.transform;
            push.ids[to] = record.id;
            for (uint w = 0; w < copy.params_words; w++) {
                push.params[to_params + w] = record.params[w];
            }
        } else {
            uint from = copy.source + i;
            push.transforms[to] = push.frame_transforms[from];
            push.ids[to] = push.frame_ids[from];
            uint from_params = copy.source_params + i * copy.params_words;
            for (uint w = 0; w < copy.params_words; w++) {
                push.params[to_params + w] = push.frame_params[from_params + w];
            }
        }
    }
}
