// Builds the view's depth pyramid, which `cull.slang` tests what the view's first pass didn't draw
// against: levels of 32-bit floats, one after another in a buffer, each a row after another, each
// texel the farthest depth, the least, of the 2 by 2 texels under it of the level before, or of the
// view's depth for the first level, as many of them as there are past the edge; so that each
// level's sides are half the one's before, rounded up, and the last level is a texel, the farthest
// depth of the whole view. `Push` matches `PyramidPush` in src/mesh/cull.rs field for field.
//
// The levels are reached as a pointer the push constants hold, indexed, as `cull.slang` says why,
// and each entry point reads its source through a function of its own, with no branch between the
// view's depth and a level: AMD's driver (25.10.30.02) compiled the levels' reads behind such a
// branch, from slangc 2026.20's SPIR-V, as reads of the index alone, without the pointer.

[[vk::binding(0, 0)]] Texture2D<float> depths[];

struct Push {
    // Every level, one after another.
    float* pyramid;
    // Where the level read starts among them, and its size; for the first level, the view's depth,
    // the size of the view, whose slot in the bindless set is `depth`.
    uint source;
    uint source_width;
    uint source_height;
    // Where the level written starts, and its size.
    uint target;
    uint target_width;
    uint target_height;
    uint depth;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// The texel of the view's depth at `at`, but within its edges.
float depth_at(uint2 at) {
    at = min(at, uint2(push.source_width - 1, push.source_height - 1));
    return depths[push.depth].Load(int3(int2(at), 0));
}

// The texel of the level read at `at`, but within its edges.
float level_at(uint2 at) {
    at = min(at, uint2(push.source_width - 1, push.source_height - 1));
    return push.pyramid[push.source + at.y * push.source_width + at.x];
}

// Whether the target has a texel at `at`.
bool inside(uint2 at) {
    return at.x < push.target_width && at.y < push.target_height;
}

// Writes the target's texel at `at`.
void write(uint2 at, float farthest) {
    push.pyramid[push.target + at.y * push.target_width + at.x] = farthest;
}

// The first level, from the view's depth.
[shader("compute")]
[numthreads(8, 8, 1)]
void depth_main(uint3 thread: SV_DispatchThreadID) {
    uint2 at = thread.xy;
    if (!inside(at)) {
        return;
    }
    uint2 under = at * 2;
    write(at, min(min(depth_at(under), depth_at(under + uint2(1, 0))),
                  min(depth_at(under + uint2(0, 1)), depth_at(under + uint2(1, 1)))));
}

// Each level after, from the one before.
[shader("compute")]
[numthreads(8, 8, 1)]
void reduce_main(uint3 thread: SV_DispatchThreadID) {
    uint2 at = thread.xy;
    if (!inside(at)) {
        return;
    }
    uint2 under = at * 2;
    write(at, min(min(level_at(under), level_at(under + uint2(1, 0))),
                  min(level_at(under + uint2(0, 1)), level_at(under + uint2(1, 1)))));
}
