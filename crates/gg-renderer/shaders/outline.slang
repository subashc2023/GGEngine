// The outline round a set of IDs, over the view's image, after outline_distances.slang. The set's
// pixels are left alone. Every other pixel takes its distance to the set: the least, over rows
// within `reach`, of the hypotenuse of the row's distance and its rows away. It draws:
// - A solid band to `width` pixels, its outer edge smoothed over the next pixel: the set's edge is
//   half a pixel past its nearest center, and this pixel's center half a pixel short of its far
//   side.
// - A glow past it, fading from `GLOW` of the band's alpha at its inner edge to nothing at twice
//   the width, by the square of the way left.

[[vk::binding(0, 0)]] Texture2D<float> distances[];

struct Push {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // The distances, in the bindless set.
    uint image;
    // The band's, in pixels: more than 0, at most 8.
    float width;
    // How far along a row the distances look, in pixels; past it they hold 1 more.
    uint reach;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// The glow's alpha at the band's inner edge, as a share of the band's.
static const float GLOW = 0.5;

// A triangle over the viewport from three vertices, no buffer: (-1, -1), (3, -1), (-1, 3) in clip.
[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    float2 corner = float2((index << 1) & 2, index & 2);
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

[shader("fragment")]
float4 fragment_main(float4 position: SV_Position) : SV_Target {
    // Positions are pixels' centers: their whole part is the pixel.
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
