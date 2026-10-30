// The arena's floor: a unit square, its corners' positions in its own space and where each is
// across it, from 0 to 1, as `Corner` in main.rs, which the struct here matches field for field;
// in one color, crossed by lines of another, `cells` across and down, as `Floor` gives them. The
// colors are linear, and the sRGB target encodes them.

struct Vertex {
    float3 position;
    float2 uv;
};

struct Params {
    float4 color;
    float4 lines;
    float2 cells;
    float width;
    float pad;
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
    Params* params;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float2 uv;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Vertex vertex = push.vertices[index];
    float4 world = mul(push.transforms[instance], float4(vertex.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.uv = vertex.uv;
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Params params = push.params[input.instance];
    // How far the point is from the nearest line, in cells, and how far a pixel goes.
    float2 cell = input.uv * params.cells;
    float2 near = abs(frac(cell + 0.5) - 0.5);
    float2 pixel = fwidth(cell);
    float2 line = 1.0 - smoothstep(params.width * pixel, (params.width + 1.0) * pixel, near);
    Targets out;
    out.color = lerp(params.color, params.lines, max(line.x, line.y));
    out.id = input.id;
    return out;
}
