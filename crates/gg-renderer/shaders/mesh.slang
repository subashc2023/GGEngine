// What the engine's mesh shaders share, included by sprite.slang, circle.slang, line.slang,
// shading.slang and clusters.slang: the bindless set, the quad's corners, the frame's data, the
// push constants and what fragments return. Each struct matches its Rust one field for field:
// - `Corner`: `Corner` in src/sprite.rs.
// - `Light`: `GpuLight` in src/light.rs.
// - `Material`: `Entry` in src/model.rs.
// - `Shadows`: `ShadowData` in src/shadow.rs.
// - `Clusters`: `ClusterData` in src/cluster.rs.
// - `Fog`: `FogData` in src/fog.rs.
// - `Aerial`: `AerialData` in src/atmosphere.rs.
// - `Reflected`: `ReflectionData` in src/reflections.rs.
// - `Push` and `Frame`: `Push` and `FrameData` in src/mesh.rs.

// The bindless set: cube textures, as the sky's prefiltered light is, through `cubes`; the sampler
// that compares depth, `COMPARISON_SAMPLER`, through `comparisons`.
[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(0, 0)]] TextureCube cubes[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(1, 0)]] SamplerComparisonState comparisons[];

// Its index in the bindless set.
static const uint COMPARISON_SAMPLER = 9;

struct Corner {
    float3 position;
    float2 uv;
};

// A light, as KHR_lights_punctual describes them.
struct Light {
    // In the world: a point's or a spot's.
    float3 position;
    // 0 for directional, 1 for point, 2 for spot.
    uint kind;
    // The way it shines, a unit vector in the world: a directional's or spot's.
    float3 direction;
    // 1 over the distance past which it lights nothing, or 0 for as far as its falloff takes it.
    float inverse_range;
    // Its linear RGB times its intensity, in lux for a directional light and candela for the rest.
    float3 color;
    // A spot's cosine from its direction is scaled, then offset, by these, then clamped to 0 to 1
    // and squared; 0 and 1 for the rest.
    float angle_scale;
    float angle_offset;
};

// A model's material, as glTF's metallic-roughness materials describe them, or
// KHR_materials_pbrSpecularGlossiness's.
struct Material {
    // Linear RGBA, alpha not multiplied in: a specular-glossiness material's diffuse color.
    float4 color;
    // Linear RGB, as the view shows it.
    float3 emissive;
    // A metallic-roughness material's, 0 for the other.
    float metallic;
    float roughness;
    float normal_scale;
    float occlusion_strength;
    // The alpha below which the masked shader discards.
    float cutoff;
    // 1 if it's unlit, 2 if it has a normal map, and 4 if it's a specular-glossiness material.
    uint flags;
    // Each texture's index and its sampler's: base color, surface (metallic-roughness or
    // specular-glossiness), normal, occlusion and emissive maps.
    uint2 textures[5];
    // A specular-glossiness material's specular color, linear RGB, and glossiness, 0 for the other.
    float3 specular;
    float glossiness;
};

static const uint CASCADES = 4;

// A directional light's shadows, in cascades, each a shadow map, nearest the view first: boxes in
// the light's space round spheres holding slices of the view's depth.
struct Shadows {
    // From the world to the light's space, its -Z the way the light shines.
    float4x4 light_from_world;
    // Each cascade's center in the light's space and 1 over its radius: `p` there is at
    // `(p - center) / radius` in its box, -1 to 1. X and Y map to its map's U and V, V pointing
    // down; Z to depth, 0 at the far side to 1 at the light's.
    float4 cascades[CASCADES];
    // Each cascade's shadow map, in the bindless set, sampled by `COMPARISON_SAMPLER`.
    uint maps[CASCADES];
    // How far a point `p` is ahead of the view: `dot(view_depth.xyz, p) + view_depth.w`.
    float4 view_depth;
    // The light that casts them, by its index among the frame's, or 0xFFFFFFFF for none.
    uint light;
    // How far ahead of the view they start to fade out, and how far they reach.
    float fade_from;
    float reach;
    // The texels across and down each map.
    float map_size;
};

// The view's clusters for the frame's lights, binned by clusters.slang and found by shading.slang's
// `cluster`: `across` by `down` tiles, each cut into `slices` by depth.
struct Clusters {
    // How many of the frame's lights, the first, light everywhere, unbinned: directional lights and
    // those without a range. The rest are binned.
    uint everywhere;
    // Words per cluster, a bit per binned light, its first the first word's lowest. 0 without
    // clusters, where fragments shade by every light.
    uint words;
    // Where, in words from the lights' address, `(uint*)lights`, each cluster's first word holding
    // a bit and the word past its last start, then each cluster's words, by index
    // `(slice * down + y) * across + x`. They follow the lights in one buffer, through one address:
    // AMD's driver (25.10.30) drops the address of a second pointer read from memory, as a
    // fragment's loops over a cluster's lights read them, and reads through the index alone.
    uint bits;
    // A point `p`'s tile across and down from the view's top left, and its depth ahead of the view:
    // `dot(x, q) / dot(w, q)`, `dot(y, q) / dot(w, q)` and `dot(depth, q)`, `q = float4(p, 1.0)`.
    float4 x;
    float4 y;
    float4 w;
    float4 depth;
    // Its slice, from that depth: `log2(depth) * slice_scale + slice_bias` with perspective,
    // `depth * slice_scale + slice_bias` without, the last taking what's past it.
    float slice_scale;
    float slice_bias;
    uint perspective;
    uint across;
    uint down;
    uint slices;
    uint unused;
};

// The view's height fog (fog.slang): densest low down, thinning exponentially with height, lit
// from all around and by the sun.
struct Fog {
    // The ambient light it scatters toward the eye, exposed; with a sky, times the sky's
    // `irradiance` the way the eye looks.
    float3 ambient;
    // Its extinction, per unit of the world, at `height`; 0 without fog.
    float density;
    // The sun's light it scatters toward the eye, exposed, before its phase function.
    float3 sun;
    // How fast it thins with height: its density at `height + h` is `density * exp(-falloff * h)`.
    float falloff;
    // The way toward the sun, a unit vector.
    float3 to_sun;
    float height;
    // How far from the eye it starts.
    float start;
    // The least share of the light from behind it that it lets through: 1 less its most opacity.
    float least;
    // Henyey-Greenstein's g, for the sun's light, from 0 up to 1.
    float lobe;
    uint unused;
};

// The atmosphere's aerial perspective, as aerial.slang reads it.
struct Aerial {
    // From the world to clip space, unjittered, where a point falls among the froxels.
    float4x4 clip_from_world;
    // The froxels, bindless, their slices side by side.
    uint texture;
    // What their light is multiplied by before the exposure: the sun's illuminance; 0 without an
    // atmosphere.
    float luminance;
};

// The view's reflections, as reflections.slang traces them and shading.slang's `reflection` reads
// them.
struct Reflected {
    // A bindless texture of the view's size, in levels: each pixel's mirror-ray light, exposed,
    // times its trust, and the trust in alpha; each level the mean of the 2 by 2 under it. 0
    // without them, and in every pass but those shading the view.
    uint colors;
    // A bindless texture of the view's size: in red, how far each pixel's reflection spreads, in
    // pixels per unit of the lobe angle's tangent.
    uint spread;
    // The roughness past which a surface takes the sky's specular light alone.
    float roughness;
    uint unused;
};

// What every draw's shader may read of the frame.
struct Frame {
    // Where the view sees from, in the world.
    float3 camera;
    // The time the view shows, in seconds.
    float time;
    // What lit shading multiplies light by.
    float exposure;
    uint light_count;
    Light* lights;
    // Every model's material, by the index a draw's parameters carry.
    Material* materials;
    // Where lit shading sees from: the camera's position and 1 for a perspective, or the way back
    // toward it and 0 for an orthographic projection. The way to the eye from `p` is
    // `eye.xyz - p * eye.w`.
    float4 eye;
    // The sky's equirectangular image, bindless, shown behind the world. 0 under an atmosphere,
    // which the view draws from its own tables; the light below is then the atmosphere's.
    uint sky;
    // The sky's prefiltered light, bindless cubes: `irradiance`, over π, for Lambert's diffuse by
    // the normal; `specular` for GGX's by the reflection, `specular_levels` levels from roughness 0
    // to 1. `specular_levels` is 0 without a sky.
    uint irradiance;
    uint specular;
    uint specular_levels;
    // The split sum's table, bindless: by the view angle's cosine across and roughness down, GGX's
    // reflectance as F0's scale in red and its bias in green.
    uint brdf;
    // The luminance, in cd/m², that a sky texel of 1 stands for, before the exposure.
    float sky_luminance;
    // The shadows of one of its lights, if one casts them.
    Shadows shadows;
    Clusters clusters;
    // The view's ambient occlusion, bindless, of the view's size: in red, the share of ambient
    // light reaching each pixel, loaded at the fragment's pixel. 0 for none, which reads as 1.
    uint ambient_occlusion;
    // What the view shows in place of its light, numbered by shading.slang's `SHOW_` constants. 0,
    // `SHOW_LIGHT`, is its light, as games' shaders take it.
    uint show;
    Fog fog;
    Aerial aerial;
    Reflected reflections;
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
    P* params;
    // The view's size in pixels.
    float2 view;
    Frame* frame;
    // For a draw of kept meshlets: the culling's draw in its first four, then each kept meshlet's
    // instance and index in the mesh.
    uint2* meshlets;
    // For their draw into the visibility buffer: its culling run and view pass, as `Push::draw` in
    // src/mesh.rs says; 0 for any other draw.
    uint draw;
    // How many transforms past each instance's is its transform the frame before: 0 where the
    // engine has none, reading the instance's own.
    uint previous;
};

// The color, and the instance's ID, which picks read back.
struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};
