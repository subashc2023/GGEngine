// Fogs the view's colors by the frame's fog, as fog.slang says, a pixel a thread, after what's
// opaque draws and before what blends. Each pixel's point comes from the view's depth, as drawn,
// offset by antialiasing. Where depth is 0, the farthest (sky or clear color), the pixel's way is
// fogged endlessly. `FogPush` matches `FogPush` in src/fog.rs field for field.

#include "mesh.slang"
#include "fog.slang"

[[vk::binding(0, 0)]] Texture2D<float> depths[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

struct FogPush {
    // From this frame's clip space to the world, homogeneous.
    float4x4 world_from_clip;
    Frame* frame;
    // The view's size in pixels.
    uint2 size;
    // Its depth, a bindless texture, and its colors, a bindless storage image.
    uint depth;
    uint target;
};
[[vk::push_constant]] ConstantBuffer<FogPush> push;

// How many pixels across and down a workgroup takes: `GROUP` in src/fog.rs.
static const uint GROUP = 8;

[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void fog_main(uint3 pixel: SV_DispatchThreadID) {
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    Frame frame = *push.frame;
    float depth = depths[push.depth].Load(int3(int2(pixel.xy), 0));
    float2 clip = (float2(pixel.xy) + 0.5) / float2(push.size) * 2.0 - 1.0;
    float4 fog;
    if (depth > 0.0) {
        float4 point = mul(push.world_from_clip, float4(clip, depth, 1.0));
        fog = fog_at(frame, point.xyz / point.w);
    } else {
        // The pixel's way, through its point at the near plane, reversed depth 1.
        float4 near = mul(push.world_from_clip, float4(clip, 1.0, 1.0));
        ViewRay ray = view_ray(frame, near.xyz / near.w);
        fog = fog_along(frame, ray.origin, ray.way, 0.0, true);
    }
    uint3 at = uint3(pixel.xy, 0);
    storage[push.target][at] = fogged(fog, storage[push.target][at]);
}
