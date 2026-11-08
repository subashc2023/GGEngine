// The showcase's own shader, which `shader.ron`'s squares draw with: a unit square, its corners'
// positions in its own space and where each is across it, from 0 to 1, as `Corner` in main.rs,
// which the struct here matches field for field; colored by waves that drift with the frame's time,
// `cells` of them across, at `speed`, tinted by `tint`, as `Plasma` gives them. The colors are
// linear, as the view's are. Save a change to it while the showcase runs, and it reloads.

struct Vertex {
    float3 position;
    float2 uv;
};

struct Params {
    float4 tint;
    float cells;
    float speed;
    float2 pad;
};

// The frame's data, as far as its time: the engine's has more after it.
struct Frame {
    float3 camera;
    float time;
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
    Params* params;
    float2 view;
    Frame* frame;
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
    float t = push.frame.time * params.speed;
    float2 p = (input.uv - 0.5) * params.cells * 6.2831853;
    // Three waves across, down and around the middle, summed, give each point its phase.
    float wave = sin(p.x + t) + sin(p.y * 0.8 - t * 1.3) + sin(length(p) - t * 0.7);
    float3 color = 0.5 + 0.5 * cos(wave + float3(0.0, 2.1, 4.2));
    Targets out;
    out.color = float4(color * color * params.tint.rgb, 1.0);
    out.id = input.id;
    return out;
}
