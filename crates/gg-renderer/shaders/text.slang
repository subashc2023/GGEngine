// Glyphs: a font's letters on the engine's quad, each a rectangle of its font's multi-channel
// distance field, whose RGB median keeps corners sharp. Filled by how far inside each pixel is,
// smoothed over a pixel at any size. An instance per glyph, with its own transform, ID and
// parameters; fully transparent fragments write no ID, so picks between strokes find what's
// behind. Fogged at its corners, as fog.slang says. `Glyph` matches `Params` in src/text.rs byte
// for byte, `rect` as `top_left` then `bottom_right`.

#include "common.slang"
#include "mesh.slang"
#include "fog.slang"

struct Glyph {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // In texture coordinates.
    float2 top_left;
    float2 bottom_right;
    uint field;
    // The distances' reach in field texels: -range / 2 at 0 to range / 2 at 1.
    float range;
};

[[vk::push_constant]] ConstantBuffer<Push<Corner, Glyph>> push;

struct Varyings {
    float4 position : SV_Position;
    // The fog at its corners, from fog.slang's `fog_at`.
    float4 fog;
    float2 uv;
    // The distances' reach in texture coordinates, across and down.
    nointerpolation float2 range;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Corner corner = push.vertices[index];
    Glyph glyph = push.params[instance];
    float4 world = mul(push.transforms[instance], float4(corner.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    Frame frame = *push.frame;
    out.fog = fog_at(frame, world.xyz);
    out.uv = lerp(glyph.top_left, glyph.bottom_right, corner.uv);
    uint width, height;
    textures[NonUniformResourceIndex(glyph.field)].GetDimensions(width, height);
    out.range = glyph.range / float2(width, height);
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

float median(float3 v) {
    return max(min(v.r, v.g), min(max(v.r, v.g), v.b));
}

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Glyph glyph = push.params[input.instance];
    // A draw's glyphs can be of their own fonts, so the index can differ across it. Read as
    // stored: the field is data.
    Texture2D field = textures[NonUniformResourceIndex(glyph.field)];
    float3 distances = field.Sample(samplers[LINEAR_CLAMP], input.uv).rgb;
    // The view's pixels the range spans here, at least one: glyphs smaller than the field blur
    // rather than alias.
    float2 texels_per_pixel = fwidth(input.uv);
    float2 pixels = input.range / max(texels_per_pixel, 1e-9);
    float range = max(0.5 * (pixels.x + pixels.y), 1.0);
    // In pixels, inward from the glyph's edge.
    float inside = (median(distances) - 0.5) * range;
    float4 color = glyph.color;
    color.a *= clamp(inside + 0.5, 0.0, 1.0);
    // Its ID would hide what's behind from picks. With no depth written, discarding keeps early
    // depth tests.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = fogged(input.fog, color);
    out.id = input.id;
    return out;
}
