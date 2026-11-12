// An outline round what the game's view shows of a set of IDs, drawn into the view's image over its
// draws, the second of two passes (outline_distances.slang is the first), from the distances it
// wrote: how far along its row each pixel is from the nearest pixel of the set.
//
// The pixels of the set are left alone, and every other pixel is covered by the distance `d` from
// its middle to the middle of the nearest pixel of the set, the least over the rows within the reach
// of the square root of that row's distance squared and the rows' own. The outline is a solid band
// to `width` pixels, with its outer edge smoothed over the pixel past it, since the set's edge is
// half a pixel past the middle of its nearest pixel, and this pixel's middle half a pixel short of
// the far side of it; and a glow past that, which fades from half the band's alpha at the band's
// inner edge to nothing at twice the width, by the square of the way left.

[[vk::binding(0, 0)]] Texture2D<float> distances[];

struct Push {
    // Its color, linear RGBA, alpha not multiplied in.
    float4 color;
    // The distances, a texture of the bindless set's.
    uint image;
    // How wide the band is, in pixels, from more than 0 to 8.
    float width;
    // How far along a row the distances look, in pixels: they hold 1 more past it.
    uint reach;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// How much alpha the glow has at the band's inner edge, as a share of the band's.
static const float GLOW = 0.5;

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
    uint columns, rows;
    distances[push.image].GetDimensions(columns, rows);
    float nearest = distances[push.image].Load(int3(texel, 0));
    if (nearest == 0.0) {
        discard;
    }
    // From the row itself outward: a row `away` rows off is no nearer than `away`.
    for (int away = 1; away <= int(push.reach) && float(away) < nearest; away++) {
        for (int side = -1; side <= 1; side += 2) {
            int row = texel.y + side * away;
            if (row < 0 || row >= int(rows)) {
                continue;
            }
            float along = distances[push.image].Load(int3(texel.x, row, 0));
            nearest = min(nearest, sqrt(along * along + float(away * away)));
        }
    }
    float band = saturate(push.width + 1.0 - nearest);
    float left = 1.0 - saturate((nearest - push.width) / push.width);
    float alpha = max(band, GLOW * left * left) * push.color.a;
    if (alpha <= 0.0) {
        discard;
    }
    return float4(push.color.rgb, alpha);
}
