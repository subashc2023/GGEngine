// What the engine's mesh shaders share, which sprite.slang, circle.slang, line.slang, shading.slang
// and clusters.slang include: the bindless set's arrays, the quad's corners, the frame's data, with
// its lights, the table of models' materials, the sky's prefiltered light, the shadows' cascades,
// the clusters its lights are binned into, its ambient occlusion, its fog, its atmosphere's haze
// and its reflections, and the push constants, for a shader's types of vertices and parameters,
// and what the fragment stage returns. `Corner` in src/sprite.rs, `GpuLight` in src/light.rs,
// `Entry` in src/model.rs, `ShadowData` in src/shadow.rs, `ClusterData` in src/cluster.rs,
// `FogData` in src/fog.rs, `AerialData` in src/atmosphere.rs, `ReflectionData` in
// src/reflections.rs and `Push` and `FrameData` in src/mesh.rs, which `Corner`, `Light`,
// `Material`, `Shadows`, `Clusters`, `Fog`, `Aerial`, `Reflected`, `Push` and `Frame` here match
// field for field.

// The bindless set's textures, those of cubes, as the sky's prefiltered light is, through the
// second; and its samplers, the one that compares depth, `COMPARISON_SAMPLER`, through the second.
[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(0, 0)]] TextureCube cubes[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(1, 0)]] SamplerComparisonState comparisons[];

// `COMPARISON_SAMPLER`'s index in the bindless set.
static const uint COMPARISON_SAMPLER = 9;

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
    // Each texture's index and its sampler's: the base color's, the surface's (the
    // metallic-roughness map, or the specular-glossiness map), the normal map's, the occlusion
    // map's and the emissive's.
    uint2 textures[5];
    // A specular-glossiness material's specular color, linear RGB, and glossiness, 0 for the other.
    float3 specular;
    float glossiness;
};

// How many cascades a light's shadows are in.
static const uint CASCADES = 4;

// The shadows a directional light casts, in cascades, each a shadow map, from the nearest the view
// out: the boxes in the light's space round spheres that hold slices of the view's depth.
struct Shadows {
    // From the world to the light's space, its -Z the way the light shines.
    float4x4 light_from_world;
    // Each cascade's center in the light's space, and 1 over its radius: a point `p` there is at
    // `(p - center) / radius` in its box, -1 to 1 each way, whose X and Y map onto its map's U, and
    // V, which points down, and whose Z onto its depth, from 0 at the far side to 1 at the light's.
    float4 cascades[CASCADES];
    // Each cascade's shadow map, a texture of the bindless set's, which `COMPARISON_SAMPLER`
    // samples.
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

// The clusters of the game's view that the frame's lights are binned into, as clusters.slang bins
// them, and model.slang's `cluster` finds a point's: the view cut into `across` tiles across and
// `down` down, each cut into `slices` along its depth.
struct Clusters {
    // How many of the frame's lights, the first, light everywhere, which no cluster bins:
    // directional lights, and those without a range. The rest are binned.
    uint everywhere;
    // Each cluster's words, a light binned a bit, its first light's the first word's lowest; 0 in a
    // frame without clusters, whose fragments each shade by every light.
    uint words;
    // Where, in words from the lights' address, `(uint*)lights`, each cluster's first word that
    // holds a bit and the word past its last start, followed by each cluster's words, by its index,
    // `(slice * down + y) * across + x`. They follow the lights in one buffer, reached through one
    // address: AMD's driver (25.10.30) drops the address of a second pointer read from memory, as
    // a fragment's loops over a cluster's lights read them, and reads through the index alone.
    uint bits;
    // Where a point `p` is across and down the tiles, from the view's top left, and how far ahead
    // of the view it is: `dot(x, q) / dot(w, q)`, `dot(y, q) / dot(w, q)` and `dot(depth, q)`, of
    // `q = float4(p, 1.0)`.
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

// The height fog the frame's view is in, as fog.slang fogs light by it, `FogData` in src/fog.rs:
// densest low down, thinning exponentially with height, lit from all around and by the sun.
struct Fog {
    // The light it scatters toward the eye of the light from all around, exposed: in a frame with
    // a sky, times the sky's diffuse light, `irradiance`, the way the eye looks.
    float3 ambient;
    // Its extinction, per unit of the world, at `height`: 0 in a frame without fog, which fogs
    // nothing.
    float density;
    // The light it scatters toward the eye of the sun's, exposed, before its phase function.
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

// The atmosphere's aerial perspective, as aerial.slang reads it: `AerialData` in
// src/atmosphere.rs, field for field.
struct Aerial {
    // From the world to clip space, unjittered, where a point falls among the froxels.
    float4x4 clip_from_world;
    // The froxels, a texture of the bindless set's, their slices side by side.
    uint texture;
    // What their light is multiplied by, before the exposure: the sun's illuminance; 0 in a frame
    // without an atmosphere.
    float luminance;
};

// The game's view's reflections, as reflections.slang traces them and shading.slang's `reflection`
// reads them: `ReflectionData` in src/reflections.rs, field for field.
struct Reflected {
    // The view's reflections, a texture of the bindless set's of the view's size, in levels: at
    // each pixel, the light its mirror ray found, exposed, times how much it's trusted, and that
    // trust in alpha, each level the mean of the 2 by 2 under it; 0 in a frame without them, and in
    // every pass but those that shade the view.
    uint colors;
    // How far a reflection at each pixel spreads, in pixels, for each unit of the tangent of a
    // lobe's angle, a texture of the bindless set's of the view's size, in red.
    uint spread;
    // The roughness past which a surface takes the sky's specular light alone.
    float roughness;
    uint unused;
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
    // The sky's image, equirectangular, a texture of the bindless set's, which the view shows
    // behind the world; 0 under an atmosphere, which the view draws from its own tables, though
    // its light below is the atmosphere's.
    uint sky;
    // The sky's light from all around, prefiltered, cubes of the bindless set's: what Lambert's
    // diffuse takes the way of a normal, the irradiance over π; and what GGX's specular takes the
    // way of a reflection, a level for each step of roughness, from none at the first to 1 at the
    // last, of `specular_levels`, which is 0 in a frame without a sky.
    uint irradiance;
    uint specular;
    uint specular_levels;
    // The split sum's table, a texture of the bindless set's: by the cosine of the view's angle
    // across and the roughness down, GGX's reflectance as F0's scale in red and its bias in green.
    uint brdf;
    // The luminance, in candela per square meter, that a texel of the sky's of 1 stands for: what
    // its light is multiplied by, before the exposure.
    float sky_luminance;
    // The shadows of one of its lights, if one casts them.
    Shadows shadows;
    // The clusters its lights are binned into.
    Clusters clusters;
    // The game's view's ambient occlusion, a texture of the bindless set's of the view's size: the
    // fraction of the light from all around that reaches each pixel, in red, which a fragment loads
    // at its pixel; 0 for none, which reads as 1.
    uint ambient_occlusion;
    // What the view shows in place of its light, as shading.slang's `SHOW_` constants number it:
    // 0, `SHOW_LIGHT`, for its light, as games' shaders take it.
    uint show;
    // The height fog its view is in.
    Fog fog;
    // The haze its atmosphere puts between the viewer and what's lit.
    Aerial aerial;
    // The game's view's reflections.
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
    // Each instance's.
    P* params;
    // The view's size in pixels.
    float2 view;
    Frame* frame;
    // For a draw of the meshlets a view kept of a mesh: the draw as the culling wrote it, in its
    // first four, then each meshlet kept, its instance and its index among the mesh's.
    uint2* meshlets;
    // For a draw of them into the visibility buffer: which run of the culling's and which of the
    // game's view's passes it is, as `Push::draw` in src/mesh.rs says; 0 for any other draw.
    uint draw;
    // How many transforms past each instance's its transform the frame before is: 0 where the
    // engine has none, which reads the instance's own.
    uint previous;
};

// The color, and the instance's ID, which picks read back where it shows.
struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};
