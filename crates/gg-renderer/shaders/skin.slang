// Skins meshes' vertices before the frame draws them, a thread a vertex. Each reads its vertex, as
// bound, and its `SkinVertex` from the mesh's buffer, blends its joints' matrices by its weights,
// and writes it where the frame's draws read it, packed on the mesh's grid, which holds wherever
// its clips take it. A mesh's first dispatch also copies its header there, so draws read it as any
// mesh's buffer. `SkinPush` matches its namesake in src/skin.rs byte for byte.

#include "shading.slang"

// `SkinVertex` in src/packed.rs, as words: four joints, 16-bit indices into its skin's, then their
// weights, unsigned 16-bit fractions of 1.
struct SkinVertex {
    uint joints_01;
    uint joints_23;
    uint weights_01;
    uint weights_23;
};

struct SkinPush {
    // The mesh's buffer, its header first, which says where its `SkinVertex`es are.
    PackedVertex* source;
    // Where its draws read it, its header first.
    PackedVertex* target;
    // Each joint's matrix, from the mesh's space as bound to its space as drawn now.
    float4x4* joints;
    uint vertices;
    uint joint_count;
    // This dispatch's first vertex.
    uint first;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<SkinPush> push;

// A `PackedVertex`'s words.
static const uint WORDS = 6;

// Unit vector `v` folded onto an octahedron, as `octahedral` in src/packed.rs folds it: two signed
// 16-bit fractions of 1, the first in the low half.
uint folded(float3 v) {
    float3 p = v / (abs(v.x) + abs(v.y) + abs(v.z));
    float2 e = p.xy;
    if (p.z < 0.0) {
        float2 signs = float2(p.x >= 0.0 ? 1.0 : -1.0, p.y >= 0.0 ? 1.0 : -1.0);
        e = (1.0 - abs(p.yx)) * signs;
    }
    int2 halves = int2(round(clamp(e, -1.0, 1.0) * 32767.0));
    return (uint(halves.x) & 0xFFFF) | (uint(halves.y) << 16);
}

[shader("compute")]
[numthreads(64, 1, 1)]
void skin_main(uint3 thread: SV_DispatchThreadID) {
    // The header a word a thread: slangc leaves the alignment SPIR-V requires off a store of a
    // whole struct copied from an address.
    if (push.first == 0 && thread.x < HEADER * WORDS) {
        ((uint*)push.target)[thread.x] = ((uint*)push.source)[thread.x];
    }
    uint index = push.first + thread.x;
    if (index >= push.vertices) {
        return;
    }
    Header header = *(Header*)push.source;
    PackedVertex packed = push.source[HEADER + index];
    SkinVertex skin = ((SkinVertex*)((uint*)push.source + header.skin))[index];
    uint4 joints = uint4(skin.joints_01 & 0xFFFF, skin.joints_01 >> 16, skin.joints_23 & 0xFFFF,
                         skin.joints_23 >> 16);
    float4 weights = float4(skin.weights_01 & 0xFFFF, skin.weights_01 >> 16,
                            skin.weights_23 & 0xFFFF, skin.weights_23 >> 16) / 65535.0;
    PackedVertex out = packed;
    // A vertex of no weight stays where it was bound.
    if (dot(weights, float4(1.0)) > 0.0) {
        out = skinned(header, packed, joints, weights);
    }
    push.target[HEADER + index].position_xy = out.position_xy;
    push.target[HEADER + index].position_z_sign = out.position_z_sign;
    push.target[HEADER + index].normal = out.normal;
    push.target[HEADER + index].tangent = out.tangent;
    push.target[HEADER + index].uv = out.uv;
    push.target[HEADER + index].color = out.color;
}

// `packed`, carried by `joints` as `weights` say; the weights sum to more than 0.
PackedVertex skinned(Header header, PackedVertex packed, uint4 joints, float4 weights) {
    // A joint past those given counts as the last, so no vertex reads past them.
    float4x4 moved = float4x4(0.0);
    for (uint k = 0; k < 4; k++) {
        moved += weights[k] * push.joints[min(joints[k], push.joint_count - 1)];
    }
    Vertex vertex = unpack(header, packed);
    float3 position = mul(moved, float4(vertex.position, 1.0)).xyz;
    Turned turned = turned(vertex, moved);
    float3 normal = normalize(turned.normal);
    float3 tangent = normalize(turned.tangent.xyz);
    // A normal or tangent that collapsed, as weights that cancel can leave, keeps its own.
    if (!all(isfinite(normal))) {
        normal = vertex.normal;
    }
    if (!all(isfinite(tangent))) {
        tangent = vertex.tangent.xyz;
    }
    float3 steps = clamp(round((position - header.offset) / header.step), 0.0, 65535.0);
    // NaN, as a joint matrix of NaNs leaves it, goes to the grid's corner.
    uint3 at = select(isnan(steps), uint3(0), uint3(steps));
    uint sign = turned.tangent.w < 0.0 ? 0xFFFF0000u : 0x00010000u;
    PackedVertex out = packed;
    out.position_xy = at.x | (at.y << 16);
    out.position_z_sign = at.z | sign;
    out.normal = folded(normal);
    out.tangent = folded(tangent);
    return out;
}
