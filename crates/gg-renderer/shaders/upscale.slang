// The view rendered below its size, upscaled to it before the canvas draws over it.
//
// - Colors: a Catmull-Rom filter of the 4 by 4 texels round each pixel, less the corners, in five
//   bilinear samples (Jimenez, "Filmic SMAA", 2016), as TAA samples its history. It's clamped to
//   the 2 by 2 nearest texels' least and most, so a bright edge leaves no dark ring.
// - Alpha: bilinear.
// - IDs: the nearest texel's, as they don't blend.
//
// `Push` matches `UpscalePush` in src/upscale.rs.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(0, 0)]] Texture2D<uint2> pairs[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Push {
    // Bindless textures: the rendered colors and IDs.
    uint colors;
    uint ids;
    // Their size, and the view's, in pixels.
    uint2 rendered;
    uint2 size;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
    // The pipeline writes no motion, but lavapipe needs a value here: `GraphicsDesc::format` says
    // why.
    float4 motion : SV_Target2;
};

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    float2 corner = float2((index << 1) & 2, index & 2);
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

[shader("fragment")]
Targets fragment_main(float4 position: SV_Position) {
    Texture2D colors = textures[push.colors];
    SamplerState filter = samplers[LINEAR_CLAMP];
    float2 texels = float2(push.rendered);
    float2 uv = position.xy / float2(push.size);
    // The Catmull-Rom weights along each axis at the pixel's position among the texels' centers.
    float2 at = uv * texels;
    float2 center = floor(at - 0.5) + 0.5;
    float2 f = at - center;
    float2 f2 = f * f;
    float2 f3 = f2 * f;
    float2 w0 = f2 - 0.5 * (f3 + f);
    float2 w1 = 1.5 * f3 - 2.5 * f2 + 1.0;
    float2 w3 = 0.5 * (f3 - f2);
    float2 w2 = 1.0 - w0 - w1 - w3;
    float2 w12 = w1 + w2;
    float2 at0 = (center - 1.0) / texels;
    float2 at12 = (center + w2 / w12) / texels;
    float2 at3 = (center + 2.0) / texels;
    float3 sum = finite(colors.SampleLevel(filter, float2(at12.x, at0.y), 0.0).rgb) * (w12.x * w0.y)
        + finite(colors.SampleLevel(filter, float2(at0.x, at12.y), 0.0).rgb) * (w0.x * w12.y)
        + finite(colors.SampleLevel(filter, at12, 0.0).rgb) * (w12.x * w12.y)
        + finite(colors.SampleLevel(filter, float2(at3.x, at12.y), 0.0).rgb) * (w3.x * w12.y)
        + finite(colors.SampleLevel(filter, float2(at12.x, at3.y), 0.0).rgb) * (w12.x * w3.y);
    float weights = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    // The 2 by 2 texels round the pixel, whose centers `center` and `center + 1` are.
    int2 last = int2(push.rendered) - 1;
    int2 first = clamp(int2(center - 0.5), int2(0, 0), last);
    float3 least = float3(1.0e9);
    float3 most = float3(-1.0e9);
    for (int y = 0; y <= 1; y++) {
        for (int x = 0; x <= 1; x++) {
            int2 texel = min(first + int2(x, y), last);
            float3 c = finite(colors.Load(int3(texel, 0)).rgb);
            least = min(least, c);
            most = max(most, c);
        }
    }
    Targets out;
    out.color.rgb = clamp(sum / weights, least, most);
    out.color.a = colors.SampleLevel(filter, uv, 0.0).a;
    int2 nearest = min(int2(at), last);
    out.id = pairs[push.ids].Load(int3(nearest, 0));
    out.motion = float4(0.0);
    return out;
}
