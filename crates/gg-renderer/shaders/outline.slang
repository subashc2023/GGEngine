// An outline round what the game's view shows of one ID, drawn into the view's image over its
// draws, from the view's IDs: each pixel off that ID's pixels is covered as far as it's within
// `width` pixels of their edge, so that the outline follows whatever shape they make, and its outer
// edge is smooth.

[[vk::binding(0, 0)]] Texture2D<uint2> ids[];

struct Push {
    // Its color, linear RGBA, alpha not multiplied in.
    float4 color;
    // The ID, its low half first, as draws write theirs.
    uint2 id;
    // The view's IDs in the bindless set.
    uint image;
    // How far out it reaches from the ID's pixels, in pixels, from more than 0 to 8.
    float width;
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
    int2 texel = int2(position.xy);
    uint width, height;
    ids[push.image].GetDimensions(width, height);
    if (all(ids[push.image].Load(int3(texel, 0)) == push.id)) {
        discard;
    }
    // The ID's edge is half a pixel past the middle of its nearest pixel, and this pixel's middle
    // half a pixel short of the far side of it: a pixel whose middle is `width` + 1 from the middle
    // of the nearest of the ID's is just past the outline. So it looks no further.
    float reach = push.width + 1.0;
    int around = int(ceil(push.width));
    float nearest = reach;
    for (int y = -around; y <= around; y++) {
        for (int x = -around; x <= around; x++) {
            int2 at = texel + int2(x, y);
            float distance = length(float2(x, y));
            // Nothing is read past the view's edges, or further than the nearest found.
            bool inside = all(at >= 0) && at.x < int(width) && at.y < int(height);
            if (distance >= nearest || !inside) {
                continue;
            }
            if (all(ids[push.image].Load(int3(at, 0)) == push.id)) {
                nearest = distance;
            }
        }
    }
    float coverage = saturate(reach - nearest);
    if (coverage <= 0.0) {
        discard;
    }
    return float4(push.color.rgb, push.color.a * coverage);
}
