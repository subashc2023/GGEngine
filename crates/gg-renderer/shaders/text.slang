// Glyphs: a font's letters on the engine's quad, each showing a rectangle of its font's field, a
// texture of data whose red, green and blue are distances to the glyph's edges, multi-channel so
// that their median keeps its corners sharp, and filled with its color by how far inside it each
// pixel is, smoothed over a pixel however big it shows. Each instance is a glyph, with its own
// transform, ID and parameters, writing its ID where it shows but where it's fully transparent, so
// that a pick between its strokes finds what's behind it, and fogged where it is by the frame's
// fog, at its corners, as fog.slang says. `Params` in src/text.rs, which `Glyph` here matches byte
// for byte, its `rect` being `top_left` then `bottom_right`.

#include "mesh.slang"
#include "fog.slang"

struct Glyph {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // In texture coordinates.
    float2 top_left;
    float2 bottom_right;
    uint field;
    // How far the field's distances reach, in its texels: from -range / 2 at 0 to range / 2 at 1.
    float range;
};

[[vk::push_constant]] ConstantBuffer<Push<Corner, Glyph>> push;

// The engine's sampler that filters linearly and clamps to the edge, the first.
static const uint LINEAR_CLAMP = 0;

struct Varyings {
    float4 position : SV_Position;
    // What the frame's fog does to it, at its corners, as fog.slang's `fog_at` gives it.
    float4 fog;
    float2 uv;
    // How far the field's distances reach, in texture coordinates, across and down.
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
    // The glyphs of one draw can be of fonts of their own, so the index can differ across it. Read
    // as stored, since the field is data.
    Texture2D field = textures[NonUniformResourceIndex(glyph.field)];
    float3 distances = field.Sample(samplers[LINEAR_CLAMP], input.uv).rgb;
    // How many of the view's pixels the field's range spans here, from how far its coordinates go
    // from one pixel to the next, so that the edge is smoothed over a pixel at any size: at least
    // one, as glyphs drawn smaller than the field blur rather than alias.
    float2 texels_per_pixel = fwidth(input.uv);
    float2 pixels = input.range / max(texels_per_pixel, 1e-9);
    float range = max(0.5 * (pixels.x + pixels.y), 1.0);
    // In pixels, from the glyph's edge, inward.
    float inside = (median(distances) - 0.5) * range;
    float4 color = glyph.color;
    color.a *= clamp(inside + 0.5, 0.0, 1.0);
    // Blending what's fully transparent changes no pixel, but its ID would hide what's behind it
    // from picks, as between the glyph's strokes. It writes no depth, so it keeps early depth
    // testing.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = fogged(input.fog, color);
    out.id = input.id;
    return out;
}
