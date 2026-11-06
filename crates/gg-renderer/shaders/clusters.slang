// Bins the frame's lights into the game's view's clusters, as src/cluster.rs plans them: the view
// cut into `across` tiles across and `down` down, each cut into `slices` along the view's depth,
// exponentially with perspective, from its near plane to as far as the farthest light reaches, and
// evenly without, from its near plane to its far. Each light with a range, past those that light
// everywhere, is binned into each cluster whose box in the view's space its sphere reaches into, as
// a bit of the cluster's words, the lights' bits in their order; and each cluster keeps its first
// word that holds a bit and the word past its last, so that a fragment reads those alone. The
// lights are copied to the front of the clusters' buffer, where the view's shaders read them and,
// through their address, the clusters, as `Clusters` in mesh.slang says why. `BinPush` matches its
// namesake in src/cluster.rs field for field.
//
// A workgroup bins a cluster, each thread every `GROUP`th word of it; and the dispatch's threads
// copy the lights' words, each thread the word at its index among them and every word as many
// threads on. The lights, their copies and the clusters are reached as pointers the push constants
// hold, indexed, as `cull.slang` says why.

#include "mesh.slang"

// The threads of a workgroup.
static const uint GROUP = 64;

struct BinPush {
    // From the world to the view's space, a point `p` at `dot(right, p)` across, `dot(up, p)` up
    // and `dot(ahead, p)` ahead, each of `float4(p, 1.0)`.
    float4 right;
    float4 up;
    float4 ahead;
    // The frame's lights, as shaders read them, where the frame uploaded them, and where their
    // words are copied to, the front of the clusters' buffer: word by word, as slangc writes a
    // whole struct through an address without the alignment Vulkan requires.
    Light* lights;
    uint* copies;
    // Past the copies: each cluster's first word that holds a bit and the word past its last, then
    // each cluster's words, `words` each, in the order of `cluster`'s index in model.slang.
    uint* clusters;
    // The first light binned, past those that light everywhere, and how many are.
    uint first;
    uint count;
    // A cluster's words, a light a bit.
    uint words;
    uint across;
    uint down;
    uint slices;
    // How far across and up the view's space a point is at 1 across and up its tiles' normalized
    // device coordinates, whose Y may point down: at a unit ahead with perspective, and anywhere
    // without.
    float x_scale;
    float y_scale;
    // How a point's slice is found from how far ahead of the view it is:
    // `log2(depth) * scale + bias` with perspective, and `depth * scale + bias` without.
    float slice_scale;
    float slice_bias;
    uint perspective;
    // The words of every light, each `Light`'s in turn, which are copied.
    uint light_words;
};
[[vk::push_constant]] ConstantBuffer<BinPush> push;

// The first word of a cluster that holds a bit, and the word past its last, as its threads find
// them.
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
    // The tile's sides, as the view's space has them at a unit ahead, or anywhere.
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
