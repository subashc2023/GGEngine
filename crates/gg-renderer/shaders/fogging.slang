// Fogs the game's view's colors, a pixel a thread, once what's opaque in it has drawn and before
// what blends draws over it, by the frame's fog, as fog.slang says: each pixel's point found from
// the view's depth, as the view drew it, offset as antialiasing offsets it; and where the depth is
// 0, as far as depth goes, where the sky or the clear color shows, the way the pixel sees, endless.
// `FogPush` matches `FogPush` in src/fog.rs field for field.

#include "mesh.slang"
#include "fog.slang"

[[vk::binding(0, 0)]] Texture2D<float> depths[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

struct FogPush {
    // From the view's clip space, as this frame draws it, to the world, homogeneous.
    float4x4 world_from_clip;
    Frame* frame;
    // The view's size in pixels.
    uint2 size;
    // Its depth, a texture of the bindless set's, and its colors, a storage image of it.
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
        // The way the pixel sees, through its point at the near plane, where reversed depth is 1.
        float4 near = mul(push.world_from_clip, float4(clip, 1.0, 1.0));
        ViewRay ray = view_ray(frame, near.xyz / near.w);
        fog = fog_along(frame, ray.origin, ray.way, 0.0, true);
    }
    uint3 at = uint3(pixel.xy, 0);
    storage[push.target][at] = fogged(fog, storage[push.target][at]);
}
