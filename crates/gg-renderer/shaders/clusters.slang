// Bins the frame's lights into the view's clusters, as src/cluster.rs plans them: `across` by
// `down` tiles, each cut into `slices` by depth. With perspective, slices grow exponentially from
// the near plane to the farthest light's reach; without, evenly from near to far. Each light with a
// range, past those lighting everywhere, sets its bit, in light order, in each cluster whose
// view-space box its sphere reaches. Each cluster keeps its first and past-last words holding a
// bit, so fragments read those alone. The lights are copied to the front of the clusters' buffer,
// as `Clusters` in mesh.slang says why. `BinPush` matches its namesake in src/cluster.rs field for
// field.
//
// A workgroup bins a cluster, each thread every `GROUP`th word; the dispatch's threads also copy
// the lights' words, strided. Every array is a push-constant pointer, indexed, as `cull.slang`
// says why.

#include "mesh.slang"

// Threads per workgroup.
static const uint GROUP = 64;

struct BinPush {
    // From the world to the view's space: `p` is `dot(right, q)` across, `dot(up, q)` up and
    // `dot(ahead, q)` ahead, `q = float4(p, 1.0)`.
    float4 right;
    float4 up;
    float4 ahead;
    // The frame's lights as uploaded, and their copy at the front of the clusters' buffer: word by
    // word, as slangc writes a whole struct through an address without the alignment Vulkan
    // requires.
    Light* lights;
    uint* copies;
    // Past the copies: each cluster's first and past-last words holding a bit, then each cluster's
    // `words` words, in the order of shading.slang's `cluster` index.
    uint* clusters;
    // The first light binned, past those lighting everywhere, and how many are.
    uint first;
    uint count;
    // Per cluster, a bit per light.
    uint words;
    uint across;
    uint down;
    uint slices;
    // How far across and up the view's space a point at 1 across and up the tiles' normalized
    // device coordinates is, whose Y may point down: a unit ahead with perspective, anywhere
    // without.
    float x_scale;
    float y_scale;
    // A point's slice from its depth: `log2(depth) * scale + bias` with perspective,
    // `depth * scale + bias` without.
    float slice_scale;
    float slice_bias;
    uint perspective;
    // Every light's words, copied.
    uint light_words;
};
[[vk::push_constant]] ConstantBuffer<BinPush> push;

// The cluster's first and past-last words holding a bit, as its threads find them.
groupshared uint first_word;
groupshared uint end_word;

// How far ahead of the view slice `slice` starts.
float slice_start(uint slice) {
    float from = (float(slice) - push.slice_bias) / push.slice_scale;
    if (push.perspective != 0) {
        return exp2(from);
    }
    return from;
}

[shader("compute")]
[numthreads(GROUP, 1, 1)]
void bin_main(uint3 group: SV_GroupID, uint thread: SV_GroupIndex) {
    if (thread == 0) {
        first_word = push.words;
        end_word = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    uint cluster = group.x;
    uint tiles = push.across * push.down;
    uint clusters = tiles * push.slices;
    for (uint word = cluster * GROUP + thread; word < push.light_words; word += clusters * GROUP) {
        push.copies[word] = ((uint*)push.lights)[word];
    }
    uint tile = cluster % tiles;
    uint slice = cluster / tiles;
    float near = slice_start(slice);
    float far = slice_start(slice + 1);
    // The tile's sides in the view's space, a unit ahead or anywhere.
    float2 count = float2(push.across, push.down);
    float2 at = float2(tile % push.across, tile / push.across);
    float2 scale = float2(push.x_scale, push.y_scale);
    float2 a = (at / count * 2.0 - 1.0) * scale;
    float2 b = ((at + 1.0) / count * 2.0 - 1.0) * scale;
    // The box round the cluster: with perspective, round its corners at its near and far sides.
    float2 least = min(a, b);
    float2 most = max(a, b);
    if (push.perspective != 0) {
        least = min(min(a * near, a * far), min(b * near, b * far));
        most = max(max(a * near, a * far), max(b * near, b * far));
    }
    float3 box_least = float3(least, near);
    float3 box_most = float3(most, far);
    for (uint word = thread; word < push.words; word += GROUP) {
        uint bits = 0;
        uint lights = min(push.count - word * 32, 32);
        for (uint bit = 0; bit < lights; bit++) {
            Light light = push.lights[push.first + word * 32 + bit];
            float4 p = float4(light.position, 1.0);
            float3 seen = float3(dot(push.right, p), dot(push.up, p), dot(push.ahead, p));
            float3 off = seen - clamp(seen, box_least, box_most);
            float range = 1.0 / light.inverse_range;
            if (dot(off, off) <= range * range) {
                bits |= 1u << bit;
            }
        }
        push.clusters[2 * clusters + cluster * push.words + word] = bits;
        if (bits != 0) {
            InterlockedMin(first_word, word);
            InterlockedMax(end_word, word + 1);
        }
    }
    GroupMemoryBarrierWithGroupSync();
    if (thread == 0) {
        push.clusters[2 * cluster] = first_word;
        push.clusters[2 * cluster + 1] = end_word;
    }
}
