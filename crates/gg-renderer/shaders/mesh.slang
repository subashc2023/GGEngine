// What the engine's mesh shaders share, which sprite.slang, circle.slang, line.slang and
// model.slang include: the bindless set's arrays, the quad's corners, the frame's data and the push
// constants, for a shader's types of vertices and parameters, and what the fragment stage returns.
// `Corner` in src/sprite.rs, which `Corner` here matches field for field, and `Push` in
// src/mesh.rs, which `Push` here matches field for field.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Corner {
    float3 position;
    float2 uv;
};

// What every draw's shader may read of the frame: `FrameData` in src/mesh.rs, field for field.
struct Frame {
    // Where the view sees from, in the world.
    float3 camera;
    // The time the view shows, in seconds.
    float time;
    // What lit shading multiplies light by.
    float exposure;
};

// What the engine pushes for each instanced draw of a mesh of vertices `V`, whose instances take
// parameters `P`.
struct Push<V, P> {
    float4x4 clip_from_world;
    V* vertices;
    // Each instance's, from its own space to the world's.
    float4x4* transforms;
    // Each instance's: its entity's, or 0.
    uint2* ids;
    // Each instance's.
    P* params;
    // The view's size in pixels.
    float2 view;
    Frame* frame;
};

// The color, and the instance's ID, which picks read back where it shows.
struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};
