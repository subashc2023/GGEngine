// Sprites: the engine's quad showing a rectangle of a texture, tinted, drawn by its alpha. Corners
// are positions in its own space and where they fall in the rectangle. An instance per sprite,
// with its own transform, ID and parameters; fully transparent fragments write no ID, so picks
// find what's behind. `Sprite` matches `Params` in src/sprite.rs byte for byte, `rect` as
// `top_left` then `bottom_right`.
//
// Only the texels the rectangle covers are sampled, so a sheet's cells packed edge to edge don't
// bleed: Godot's region filter clip, which Godot makes optional and insets by half a texel. Fogged
// at its corners, as fog.slang says.

#include "mesh.slang"
#include "fog.slang"

struct Sprite {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // In texture coordinates.
    float2 top_left;
    float2 bottom_right;
    uint texture;
    uint sampler;
};

[[vk::push_constant]] ConstantBuffer<Push<Corner, Sprite>> push;

struct Varyings {
    float4 position : SV_Position;
    // The fog at its corners, from fog.slang's `fog_at`.
    float4 fog;
    float2 uv;
    // Where the fragment stage may sample, in texture coordinates: least corner, then greatest.
    nointerpolation float4 inside;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

// How far a texture coordinate times its texture's size can round off, in texels: about a
// thousandth at 8,192. An edge that near a texel's is taken as on it, and the bounds keep that far
// inside texel centers, which a sampler truncating within a texel could cross.
static const float ROUNDING = 1.0 / 256.0;

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Corner corner = push.vertices[index];
    Sprite sprite = push.params[instance];
    float4 world = mul(push.transforms[instance], float4(corner.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    Frame frame = *push.frame;
    out.fog = fog_at(frame, world.xyz);
    out.uv = lerp(sprite.top_left, sprite.bottom_right, corner.uv);
    // The centers of the outermost texels the rectangle touches: between them neither a linear
    // sampler (2 by 2 texels) nor a nearest one reaches past its edges. A rectangle within a texel
    // samples its center. The max keeps the bounds ordered, as clamp needs, for one of no width on
    // a texel's edge; the nudge stops halfway.
    uint width, height;
    textures[NonUniformResourceIndex(sprite.texture)].GetDimensions(width, height);
    float2 size = float2(width, height);
    float2 least = floor(min(sprite.top_left, sprite.bottom_right) * size + ROUNDING) + 0.5;
    float2 most = ceil(max(sprite.top_left, sprite.bottom_right) * size - ROUNDING) - 0.5;
    float2 nudge = clamp((most - least) / 2.0, 0.0, ROUNDING);
    out.inside = float4(least + nudge, max(least, most) - nudge) / float4(size, size);
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Sprite sprite = push.params[input.instance];
    // A draw's sprites sample their own textures, so the index can differ across it. Sampling
    // decodes sRGB to linear.
    Texture2D texture = textures[NonUniformResourceIndex(sprite.texture)];
    SamplerState sampler = samplers[NonUniformResourceIndex(sprite.sampler)];
    float2 uv = clamp(input.uv, input.inside.xy, input.inside.zw);
    float4 color = texture.Sample(sampler, uv) * sprite.color;
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
