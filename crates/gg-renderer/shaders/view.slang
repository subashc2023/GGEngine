// The game's view, drawn into the frame where it shows: each pixel of the view's image onto one of
// the frame's, as it is, which the fetch decodes from sRGB and the target encodes again.

[[vk::binding(0, 0)]] Texture2D textures[];

struct Push {
    // Where the view's top left is in the target, in pixels.
    uint2 origin;
    // The view's image in the bindless set.
    uint image;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// A triangle over the whole viewport, from three vertices and no buffer: (-1, -1), (3, -1) and
// (-1, 3) in clip space.
[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    float2 corner = float2((index << 1) & 2, index & 2);
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

[shader("fragment")]
float4 fragment_main(float4 position: SV_Position) : SV_Target {
    // A pixel's position is its center, so its whole part is the pixel's.
    int2 texel = int2(position.xy) - int2(push.origin);
    return textures[push.image].Load(int3(texel, 0));
}
