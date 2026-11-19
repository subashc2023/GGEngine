// The sky behind the world: each pixel the sky's equirectangular image the way it sees, times its
// luminance and the exposure. Drawn first, at depth 0 (the farthest), writing no ID. `Push`
// matches `Push` in src/sky.rs field for field.

#include "common.slang"
#include "equirect.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Push {
    // From clip space to the ways the view sees, turned back by the sky's rotation into the
    // image's, as points whose way from the origin is the way. With perspective, the inverse of the
    // projection and rotation, without translation; for an orthographic projection, a matrix taking
    // every point to its one way.
    float4x4 world_from_clip;
    // The sky's texture in the bindless set.
    uint sky;
    // Multiplies its colors.
    float scale;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float2 clip;
};

// A triangle over the viewport, from three vertices and no buffer, at depth 0.
[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID) {
    float2 corner = float2((index << 1) & 2, index & 2) * 2.0 - 1.0;
    Varyings out;
    out.position = float4(corner, 0.0, 1.0);
    out.clip = corner;
    return out;
}

// The color, no ID, and zero motion: the pipeline doesn't write motion, but lavapipe may, as
// `GraphicsDesc::format` in gg-gpu says.
struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
    float4 motion : SV_Target2;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    // The near plane, reversed depth 1, which perspective's inverse takes to a point.
    float4 point = mul(push.world_from_clip, float4(input.clip, 1.0, 1.0));
    float3 way = normalize(point.xyz / point.w);
    Texture2D sky = textures[push.sky];
    uint width, height;
    sky.GetDimensions(width, height);
    float3 color = equirect(sky, samplers[LINEAR_REPEAT], float2(width, height), way);
    Targets out;
    out.color = float4(min(color * push.scale, BRIGHTEST), 1.0);
    out.id = uint2(0, 0);
    out.motion = float4(0.0);
    return out;
}
