// Gathers a frame's opaque runs' instances, run after run, into the arrays `cull.slang` culls. The
// frame's draws come from where the frame copied them; the scene's, kept on the GPU from frame to
// frame, from its table of records, in the scene's order. Last frame's transforms go past the
// transforms; the frame's draws, made anew each frame, repeat their own. `Copy`, `Record` and
// `Push` match `GpuCopy`, `Record` in src/mesh/scene.rs and `GatherPush` in src/mesh/cull.rs field
// for field. Arrays are indexed pointers from the push constants, as `cull.slang` says why.

// A run's instances per copy, a workgroup's: `BLOCK` in src/mesh/cull.rs.
static const uint BLOCK = 64;
// A record's parameters: `MeshDraw::MAX_PARAMS` in src/mesh.rs, in words.
static const uint RECORD_WORDS = 16;
// A copy's `source_params` for one from the scene.
static const uint SCENE = 0xFFFFFFFF;

// Up to `BLOCK` instances of a run, a workgroup's to copy.
struct Copy {
    // Where the first goes in the arrays, and its parameters' first word there.
    uint dest;
    uint dest_params;
    // How many, and each one's parameter words.
    uint count;
    uint params_words;
    // Where the first is: in the frame's arrays, its parameters from `source_params`; or, where
    // that's `SCENE`, in the scene's order, which holds each one's slot in its table.
    uint source;
    uint source_params;
};

// A scene instance in its table slot: its transform, and `previous`, the one it had as frame
// `changed` (low word first) began, the frame its transform last changed in.
struct Record {
    float4x4 transform;
    float4x4 previous;
    uint2 id;
    uint2 changed;
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
    // Where they go, for `cull.slang`: last frame's transforms `instances` past.
    float4x4* transforms;
    uint2* ids;
    uint* params;
    uint copy_count;
    // Workgroups dispatched across; each takes every `groups`th copy.
    uint groups;
    // The frame's number, low word first.
    uint2 frame;
    uint instances;
    uint unused;
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
            // Read whole, as the model shaders read the frame's data: every address is a pushed
            // one indexed, none kept across the loop.
            Record record = push.scene[push.order[copy.source + i]];
            push.transforms[to] = record.transform;
            // Where the frame before drew it: `previous` if its transform changed this frame,
            // else where it is.
            bool changed = all(record.changed == push.frame);
            push.transforms[push.instances + to] = changed ? record.previous : record.transform;
            push.ids[to] = record.id;
            for (uint w = 0; w < copy.params_words; w++) {
                push.params[to_params + w] = record.params[w];
            }
        } else {
            uint from = copy.source + i;
            float4x4 transform = push.frame_transforms[from];
            push.transforms[to] = transform;
            push.transforms[push.instances + to] = transform;
            push.ids[to] = push.frame_ids[from];
            uint from_params = copy.source_params + i * copy.params_words;
            for (uint w = 0; w < copy.params_words; w++) {
                push.params[to_params + w] = push.frame_params[from_params + w];
            }
        }
    }
}
