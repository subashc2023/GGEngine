// What the engine's mesh shaders share, which sprite.slang, circle.slang, line.slang and
// model.slang include: the bindless set's arrays, the quad's corners, the frame's data, with its
// lights and the table of models' materials, and the push constants, for a shader's types of
// vertices and parameters, and what the fragment stage returns. `Corner` in src/sprite.rs,
// `GpuLight` in src/light.rs, `Entry` in src/model.rs and `Push` in src/mesh.rs, which `Corner`,
// `Light`, `Material` and `Push` here match field for field.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Corner {
    float3 position;
    float2 uv;
};

// A light, as KHR_lights_punctual describes them.
struct Light {
    // Where it shines from, in the world: a point's or a spot's.
    float3 position;
    // 0 for directional, 1 for point, 2 for spot.
    uint kind;
    // The way it shines, a unit vector in the world: a directional's or a spot's.
    float3 direction;
    // 1 over the distance past which it lights nothing, or 0 for as far as its falloff takes it.
    float inverse_range;
    // Its linear RGB times its intensity, in lux for a directional light and candela for the rest.
    float3 color;
    // What a spot's cosine from its direction is scaled by, then offset by, before it's clamped to
    // 0 to 1 and squared: 0 and 1 for the rest.
    float angle_scale;
    float angle_offset;
};

// A model's material, as glTF's metallic-roughness materials describe them.
struct Material {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // Linear RGB, as the view shows it.
    float3 emissive;
    float metallic;
    float roughness;
    float normal_scale;
    float occlusion_strength;
    // The alpha below which the masked shader discards.
    float cutoff;
    // 1 if it's unlit, and 2 if it has a normal map.
    uint flags;
    // Each texture's index and its sampler's: the base color's, the metallic-roughness map's, the
    // normal map's, the occlusion map's and the emissive's.
    uint2 textures[5];
};

// What every draw's shader may read of the frame: `FrameData` in src/mesh.rs, field for field.
struct Frame {
    // Where the view sees from, in the world.
    float3 camera;
    // The time the view shows, in seconds.
    float time;
    // What lit shading multiplies light by.
    float exposure;
    // How many lights there are, and where.
    uint light_count;
    Light* lights;
    // Every model's material, by its index, which a model's draw's parameters carry.
    Material* materials;
    // Where lit shading sees from: the camera's position and 1 for a perspective, or the way back
    // toward it and 0 for an orthographic projection, so that the way to the eye from `p` is
    // `eye.xyz - p * eye.w`.
    float4 eye;
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
