// egui's meshes, drawn over the frame into an sRGB target: egui-wgpu's shader for egui 0.37
// (`fs_main_linear_framebuffer`), without its dithering. egui's colors and textures are
// sRGB-encoded and premultiplied; they're multiplied together encoded, as egui expects, and
// decoded on output, which the target encodes again.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Push {
    // egui's vertices, five words each: position and texture coordinates as two floats each,
    // in points, then the color as four bytes.
    uint* vertices;
    // The target's size in points.
    float2 screen_size;
    uint texture;
    uint sampler;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float2 uv;
    float4 color;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID) {
    uint* vertex = push.vertices + index * 5;
    float2 position = float2(asfloat(vertex[0]), asfloat(vertex[1]));
    uint color = vertex[4];
    Varyings out;
    // Points from the top left to Vulkan's clip space, whose y points down too.
    out.position = float4(2.0 * position / push.screen_size - 1.0, 0.0, 1.0);
    out.uv = float2(asfloat(vertex[2]), asfloat(vertex[3]));
    out.color = float4(color & 255, (color >> 8) & 255, (color >> 16) & 255, color >> 24) / 255.0;
    return out;
}

float3 linear_from_srgb(float3 srgb) {
    float3 lower = srgb / 12.92;
    float3 higher = pow((srgb + 0.055) / 1.055, 2.4);
    return select(srgb < 0.04045, lower, higher);
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    float4 texel = textures[push.texture].Sample(samplers[push.sampler], input.uv);
    float4 color = input.color * texel;
    return float4(linear_from_srgb(color.rgb), color.a);
}
