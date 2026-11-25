// Shades the game's view from its visibility buffer, a pixel a thread: where a meshlet's triangle
// drew (model.slang's `seen`), what model.slang's `fragment_main` would have shaded; elsewhere,
// the view's clear color. The triangle's corners are found through the run's meshlet draw, and
// unpacked and placed as the draw placed them.
//
// The pixel weighs the corners in clip space, homogeneous in X, Y and W: a point at (x, y) in
// normalized coordinates weighs each by the determinant of the other two and the point,
// `dot(cross(b, c), (x, y, 1))`, over their sum (Olano and Greer, "Triangle Scan Conversion using
// 2D Homogeneous Coordinates", 1997). Dividing by no W, it weighs a clipped corner behind the
// camera right too. The weights a pixel across and down give the UVs' and the normal's change,
// which pick texture levels and widen roughness as a quad's derivatives would. Applied to the
// corners as placed the frame before, they give the view's motion; 0 where nothing drew.
//
// Where the view reflects, `normal_main` first finds each pixel's smooth shading normal the same
// way, for reflections.slang's rays: a curved mesh's depth facets aren't smooth.
//
// `ShadePush` and `Shading` match their namesakes in src/mesh/visibility.rs field for field. A
// change here needs AMD's ISA checked again, under both slangc releases the build may use, as
// shading.slang's light loop does.

#include "runs.slang"
#include "shading.slang"

[[vk::binding(0, 0)]] Texture2D<uint2> visibilities[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// What the shading reads of the view and its culling.
struct Shading {
    // From the world to the view's clip space, as the draws placed the corners.
    float4x4 clip_from_world;
    // What the view clears to, linear RGBA, where nothing drew.
    float4 clear;
    // The view's size in pixels.
    uint2 size;
    // Room per culling view, in instances and in their parameters' words.
    uint instances;
    uint params_words;
    // For each of the view's two passes: its slot among the culling's views, where its meshlets
    // start in the culling's commands, in words, and where its kept instances' transforms the frame
    // before are among the kept transforms' arrays.
    uint2 slots;
    uint2 meshlets;
    uint2 previous;
    uint2 unused;
};

struct ShadePush {
    Shading* shading;
    Frame* frame;
    Run* runs;
    // The culling's commands, as words: each view's meshlets where `Shading::meshlets` says, each
    // run's draw of them, eight words, then the meshlets it kept, as meshlets.slang keeps them.
    uint* commands;
    // What each of the culling's views keeps of the instances (cull.slang): their transforms, the
    // view passes' ones the frame before, and their parameters' words: model.slang's `Params`,
    // the material's index, then the tint.
    float4x4* transforms;
    uint* params;
    // The visibility buffer, bindless, and the view's colors and motion, storage images; for
    // `normal_main`, its normals, and no motion.
    uint visibility;
    uint target;
    uint motion;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<ShadePush> push;

// How many pixels across and down a workgroup takes: `GROUP` in src/mesh/visibility.rs.
static const uint GROUP = 8;

// A corner of the triangle a pixel shows, as the draw placed it.
struct Placed {
    Vertex vertex;
    float3 world;
    float4 clip;
    Turned turned;
};

// `packed`, placed as model.slang's `placed` does.
Placed placed(Header header, PackedVertex packed, float4x4 transform, float4x4 clip_from_world) {
    Placed out;
    out.vertex = unpack(header, packed);
    float4 world = mul(transform, float4(out.vertex.position, 1.0));
    out.world = world.xyz;
    out.clip = mul(clip_from_world, world);
    out.turned = turned(out.vertex, transform);
    return out;
}

// Byte `byte` of the meshlets' triangles, of the word that holds it.
uint byte_of(uint word, uint byte) {
    return (word >> ((byte & 3) * 8)) & 0xFF;
}

// What a pixel shows of its triangle. `was`: where the transform the frame before placed it.
// `uv_across` and `uv_down`: the UVs' change a pixel across and down; `normal_across` and
// `normal_down`, the unit normal's.
struct Shown {
    Material material;
    float4 tint;
    float3 world;
    float3 was;
    float2 uv;
    float2 uv_across;
    float2 uv_down;
    float4 vertex_color;
    float3 normal;
    float3 normal_across;
    float3 normal_down;
    float4 tangent;
    bool front;
};

// What `pixel` shows, where the visibility buffer holds `seen`: in x, 1 + 2 * the culling run's
// index + its view pass; in y, its meshlet and triangle. It takes the material table's address, not
// the frame: given a frame, slangc 2026 reads the table through a pointer into it, which AMD's
// driver miscompiles, as model.slang's `shadow_main` says.
Shown shown_at(Shading shading, Material* materials, uint2 pixel, uint2 seen) {
    uint drawn = seen.x - 1;
    Run run = push.runs[drawn >> 1];
    bool second = (drawn & 1) != 0;
    uint slot = second ? shading.slots.y : shading.slots.x;
    uint meshlets = second ? shading.meshlets.y : shading.meshlets.x;
    // The kept meshlet's entry: its instance's place among the run's in the view, and its index in
    // the mesh.
    uint entry = meshlets + run.meshlet_draw + 8 + 2 * (seen.y >> 7);
    uint place = push.commands[entry];
    uint meshlet = push.commands[entry + 1];
    float4x4 transform = push.transforms[slot * shading.instances + run.output + place];
    uint before = second ? shading.previous.y : shading.previous.x;
    float4x4 previous = push.transforms[before * shading.instances + run.output + place];
    uint params = slot * shading.params_words + run.params_output + place * run.params_words;
    Shown out;
    out.material = materials[push.params[params]];
    out.tint = float4(asfloat(push.params[params + 1]), asfloat(push.params[params + 2]),
                      asfloat(push.params[params + 3]), asfloat(push.params[params + 4]));
    // The mesh's buffer, as model.slang reads `push.vertices`: its address's two words cast to a
    // pointer, as a 64-bit integer would need the shaderInt64 feature.
    uint* words = bit_cast<uint*>(run.vertices);
    Header header = *(Header*)words;
    // What draws the meshlet: `GpuMeshlet` in src/packed.rs.
    uint first_vertex = words[header.meshlets + meshlet * 4];
    uint byte = words[header.meshlets + meshlet * 4 + 2] + 3 * (seen.y & 0x7F);
    uint3 local = uint3(byte_of(words[header.triangles + byte / 4], byte),
                        byte_of(words[header.triangles + (byte + 1) / 4], byte + 1),
                        byte_of(words[header.triangles + (byte + 2) / 4], byte + 2));
    uint first = header.meshlet_vertices + first_vertex;
    uint3 index = uint3(words[first + local.x], words[first + local.y], words[first + local.z]);
    float4x4 clip = shading.clip_from_world;
    Placed a = placed(header, ((PackedVertex*)words)[HEADER + index.x], transform, clip);
    Placed b = placed(header, ((PackedVertex*)words)[HEADER + index.y], transform, clip);
    Placed c = placed(header, ((PackedVertex*)words)[HEADER + index.z], transform, clip);
    // Each corner's weight at the pixel's center, and a pixel across and down.
    float3 ha = a.clip.xyw;
    float3 hb = b.clip.xyw;
    float3 hc = c.clip.xyw;
    float3 ea = cross(hb, hc);
    float3 eb = cross(hc, ha);
    float3 ec = cross(ha, hb);
    float2 step = 2.0 / float2(shading.size);
    float3 point = float3((float2(pixel.xy) + 0.5) * step - 1.0, 1.0);
    float3 here = float3(dot(ea, point), dot(eb, point), dot(ec, point));
    float3 across = here + float3(ea.x, eb.x, ec.x) * step.x;
    float3 down = here + float3(ea.y, eb.y, ec.y) * step.y;
    float3 weight = here / (here.x + here.y + here.z);
    float3 weight_across = across / (across.x + across.y + across.z) - weight;
    float3 weight_down = down / (down.x + down.y + down.z) - weight;
    // Counterclockwise as seen, Vulkan's front, is a negative determinant, as the view's Y points
    // down, whatever the W: clipping keeps winding. That's the mesh's front, or its back where the
    // transform mirrors. A run drawing fronts alone shows nothing else.
    out.front = run.cones != 0 || ((dot(ha, ea) < 0.0) != (a.turned.side < 0.0));
    out.world = weight.x * a.world + weight.y * b.world + weight.z * c.world;
    out.was = weight.x * mul(previous, float4(a.vertex.position, 1.0)).xyz
        + weight.y * mul(previous, float4(b.vertex.position, 1.0)).xyz
        + weight.z * mul(previous, float4(c.vertex.position, 1.0)).xyz;
    out.uv = weight.x * a.vertex.uv + weight.y * b.vertex.uv + weight.z * c.vertex.uv;
    out.uv_across = weight_across.x * a.vertex.uv + weight_across.y * b.vertex.uv
        + weight_across.z * c.vertex.uv;
    out.uv_down = weight_down.x * a.vertex.uv + weight_down.y * b.vertex.uv
        + weight_down.z * c.vertex.uv;
    out.vertex_color =
        weight.x * a.vertex.color + weight.y * b.vertex.color + weight.z * c.vertex.color;
    out.normal =
        weight.x * a.turned.normal + weight.y * b.turned.normal + weight.z * c.turned.normal;
    float3 unit = normalize(out.normal);
    out.normal_across = normalize(out.normal + weight_across.x * a.turned.normal
        + weight_across.y * b.turned.normal + weight_across.z * c.turned.normal) - unit;
    out.normal_down = normalize(out.normal + weight_down.x * a.turned.normal
        + weight_down.y * b.turned.normal + weight_down.z * c.turned.normal) - unit;
    out.tangent =
        weight.x * a.turned.tangent + weight.y * b.turned.tangent + weight.z * c.turned.tangent;
    return out;
}

[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void shade_main(uint3 pixel: SV_DispatchThreadID) {
    Shading shading = *push.shading;
    if (pixel.x >= shading.size.x || pixel.y >= shading.size.y) {
        return;
    }
    uint3 at = uint3(pixel.xy, 0);
    uint2 seen = visibilities[push.visibility].Load(int3(pixel.xy, 0));
    if (seen.x == 0) {
        storage[push.target][at] = shading.clear;
        storage[push.motion][at] = float4(0.0);
        return;
    }
    // Loaded whole for its addresses, as model.slang's `shadow_main` says why; the rest read
    // through `frame` where used, as `lit` says why.
    Frame whole = loadAligned<8>(push.frame);
    Frame* frame = push.frame;
    Shown shown = shown_at(shading, whole.materials, pixel.xy, seen);
    storage[push.motion][at] = float4(shown.world - shown.was, 0.0);
    Material material = shown.material;
    float2 uv = shown.uv;
    float2 uv_across = shown.uv_across;
    float2 uv_down = shown.uv_down;
    // As `fragment_main` samples them, and an opaque or masked material's alpha of 1.
    float4 color = sampled_by(material, 0, uv, uv_across, uv_down) * material.color
        * shown.vertex_color * shown.tint;
    float4 surface = sampled_by(material, 1, uv, uv_across, uv_down);
    float2 mapped = sampled_by(material, 2, uv, uv_across, uv_down).xy * 2.0 - 1.0;
    float occlusion = 1.0
        + material.occlusion_strength * (sampled_by(material, 3, uv, uv_across, uv_down).r - 1.0);
    float3 emission = sampled_by(material, 4, uv, uv_across, uv_down).rgb * material.emissive;
    color.a = 1.0;
    occlusion = ambient(frame, pixel.xy, occlusion);
    float roughness = filtered_roughness(roughness_of(material, surface), shown.normal_across,
                                         shown.normal_down);
    float4 reflected = reflection(frame, pixel.xy, roughness);
    storage[push.target][at] =
        lit(frame, whole.lights, material, shown.world, shown.normal, shown.tangent, shown.front,
            color, surface, roughness, mapped, occlusion, emission, reflected);
}

// Each pixel's unit normal in the world, as shading turns it, with its normal map's bumps, in RGB,
// and 1 in alpha, where a meshlet's triangle drew; 0 elsewhere.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void normal_main(uint3 pixel: SV_DispatchThreadID) {
    Shading shading = *push.shading;
    if (pixel.x >= shading.size.x || pixel.y >= shading.size.y) {
        return;
    }
    uint3 at = uint3(pixel.xy, 0);
    uint2 seen = visibilities[push.visibility].Load(int3(pixel.xy, 0));
    if (seen.x == 0) {
        storage[push.target][at] = float4(0.0);
        return;
    }
    // Loaded whole, as model.slang's `shadow_main` says why.
    Frame frame = loadAligned<8>(push.frame);
    Shown shown = shown_at(shading, frame.materials, pixel.xy, seen);
    float2 mapped =
        sampled_by(shown.material, 2, shown.uv, shown.uv_across, shown.uv_down).xy * 2.0 - 1.0;
    float3 surface_normal;
    float3 normal = shading_normal(shown.material, shown.normal, shown.tangent, shown.front, mapped,
                                   surface_normal);
    storage[push.target][at] = float4(normal, 1.0);
}
