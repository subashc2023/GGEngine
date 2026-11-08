// Skinned meshes' vertices, moved by their joints before the frame draws them: a thread for each
// vertex of a skinned model's mesh, which reads it from the mesh's buffer, as it was bound, with
// its `SkinVertex`, and writes it where its joints carry it, as the matrices of its joints blended
// by its weights place it, into where the frame's draws of the mesh read it, packed on the mesh's
// grid, which holds wherever its clips take it. The first threads of a mesh's first dispatch copy
// its header there too, so that a draw reads it as it reads any mesh's buffer. `SkinPush` in
// src/skin.rs matches `Push` byte for byte.

#include "shading.slang"

// What moves a vertex, `SkinVertex` in src/packed.rs, read as words: its four joints, as 16-bit
// indices of its skin's joints, then their weights, as unsigned 16-bit fractions of 1.
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
    // Each of its skin's joints' matrices, from the mesh's space as the joint was bound to the
    // mesh's space as the joint is drawn now.
    float4x4* joints;
    uint vertices;
    uint joint_count;
    // The first vertex of this dispatch's.
    uint first;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<SkinPush> push;

// The words of a `PackedVertex`.
static const uint WORDS = 6;

// `v`, a unit vector, folded onto an octahedron and unfolded onto a square, as `octahedral` in
// src/packed.rs folds it, as two signed 16-bit fractions of 1 in a word, its first in the low half.
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
    // The header a word a thread, as slangc leaves the alignment off a store of a whole struct
    // copied from an address, which SPIR-V requires.
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

// `packed`, a vertex of a mesh whose buffer starts with `header`, where `joints` carry it, each as
// much as `weights` says, which add up to more than 0.
PackedVertex skinned(Header header, PackedVertex packed, uint4 joints, float4 weights) {
    // A joint past those given counts as the last, so that no vertex reads past them.
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
    // Where it isn't a number, as a joint's matrix that isn't can leave it, at the grid's corner.
    uint3 at = select(isnan(steps), uint3(0), uint3(steps));
    uint sign = turned.tangent.w < 0.0 ? 0xFFFF0000u : 0x00010000u;
    PackedVertex out = packed;
    out.position_xy = at.x | (at.y << 16);
    out.position_z_sign = at.z | sign;
    out.normal = folded(normal);
    out.tangent = folded(tangent);
    return out;
}
