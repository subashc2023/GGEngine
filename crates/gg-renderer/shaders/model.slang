// Models' meshes, shaded as shading.slang says: each instance a mesh with its own transform, ID and
// material, writing its ID where it shows, and how far each point moved in the world since the
// frame before, by its instance's transform then, which the view's antialiasing follows it by.
// `Params` matches `Params` in src/model.rs byte for byte.
//
// As it is, it's for materials that blend by their alpha, and discards what's fully transparent,
// which blending would leave as it was, so that a pick there finds what's behind it; it writes no
// depth, so it keeps early depth testing. model_opaque.slang and model_masked.slang include it
// with OPAQUE or MASKED defined, for materials whose alpha glTF ignores, or uses only to cut out
// what's less opaque than their cutoff: both are drawn fully opaque, so they return an alpha of 1.
// Of those, only the masked one discards, since a shader that can discard while it writes depth
// loses early depth testing on some GPUs; its shadows are cut out as it is, by `shadow_main`,
// where those of an opaque material, drawn without a fragment shader, are whole.
//
// What blends fogs itself by the frame's fog, as fog.slang says, as it draws over what the fog's
// pass fogged, which fogs what's opaque.
//
// The meshlets a view keeps of their meshes draw into the visibility buffer too, which the view's
// shading then shades a pixel at a time (visibility.slang): each fragment the draw and the meshlet
// and triangle it shows, for the shading to find again, and its instance's ID, cut out as
// `fragment_main` cuts it out.

#include "shading.slang"
#include "fog.slang"

struct Params {
    // The material's index in the frame's table.
    uint material;
};

[[vk::push_constant]] ConstantBuffer<Push<PackedVertex, Params>> push;

// The mesh's vertex `index`, unpacked as its header says.
Vertex unpacked(uint index) {
    Header header = *(Header*)push.vertices;
    return unpack(header, push.vertices[HEADER + index]);
}

struct Varyings {
    float4 position : SV_Position;
    // Where it is in the world, and how far it moved there since the frame before.
    float3 world;
    float3 moved;
    float2 uv;
    float4 color;
    float3 normal;
    float4 tangent;
    // In x, -1 where the instance's transform mirrors it, 1 elsewhere. A vector, as Debian 13's
    // validation layer (1.4.309) warns of an output the next stage doesn't read, as the masked
    // shadows' `shadow_main` doesn't read this, unless it's a vector.
    nointerpolation float2 side;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    return placed(unpacked(index), instance);
}

// The most vertices and triangles a meshlet has: `MAX_MESHLET_VERTICES` and
// `MAX_MESHLET_TRIANGLES` in src/packed.rs.
static const uint MAX_VERTICES = 64;
static const uint MAX_TRIANGLES = 124;
// Where a draw's meshlets start in `push.meshlets`, past the draw's eight words.
static const uint KEPT = 4;
// How many workgroups across a draw of meshlets through a mesh shader takes: `ACROSS` in
// meshlets.slang.
static const uint ACROSS = 32768;

// Byte `byte` of the mesh's meshlets' triangles, as its header places them.
uint triangle_byte(Header header, uint byte) {
    uint word = ((uint*)push.vertices)[header.triangles + byte / 4];
    return (word >> ((byte & 3) * 8)) & 0xFF;
}

// The vertex at corner `corner` of the meshlet a view kept that the draw's `kept`th entry holds,
// drawn through vertex shaders, as `meshlet_vertex_main` draws it, by its index in the mesh; and
// its instance's place among the draw's instances.
uint2 meshlet_corner(uint corner, uint kept) {
    uint2 entry = push.meshlets[KEPT + kept];
    Header header = *(Header*)push.vertices;
    // What draws the meshlet: `GpuMeshlet` in src/packed.rs.
    uint at = header.meshlets + entry.y * 4;
    uint first_vertex = ((uint*)push.vertices)[at];
    uint first_triangle = ((uint*)push.vertices)[at + 2];
    uint triangle_count = ((uint*)push.vertices)[at + 3];
    uint local = 0;
    if (corner / 3 < triangle_count) {
        local = triangle_byte(header, first_triangle + corner);
    }
    uint index = ((uint*)push.vertices)[header.meshlet_vertices + first_vertex + local];
    return uint2(index, entry.x);
}

// A corner of a meshlet a view kept, drawn through vertex shaders: each meshlet kept an instance of
// the draw, of `MAX_TRIANGLES` triangles, whose `corner`th this is. The corners of a triangle past
// the meshlet's own are all its first vertex, which draws nothing.
[shader("vertex")]
Varyings meshlet_vertex_main(uint corner: SV_VulkanVertexID, uint kept: SV_VulkanInstanceID) {
    uint2 vertex = meshlet_corner(corner, kept);
    return placed(unpacked(vertex.x), vertex.y);
}

// A meshlet a view kept, drawn through a mesh shader: a workgroup each, `ACROSS` across, as many
// rows down as they take, those past the draw's meshlets drawing nothing; a thread for each of its
// vertices, and for each of its triangles and the one `MAX_VERTICES` after it.
[shader("mesh")]
[outputtopology("triangle")]
[numthreads(MAX_VERTICES, 1, 1)]
void mesh_main(uint3 group: SV_GroupID, uint thread: SV_GroupIndex,
               OutputVertices<Varyings, MAX_VERTICES> vertices,
               OutputIndices<uint3, MAX_TRIANGLES> triangles) {
    uint kept = group.y * ACROSS + group.x;
    // How many the draw has, its second word.
    if (kept >= push.meshlets[0].y) {
        SetMeshOutputCounts(0, 0);
        return;
    }
    uint2 entry = push.meshlets[KEPT + kept];
    Header header = *(Header*)push.vertices;
    uint at = header.meshlets + entry.y * 4;
    uint first_vertex = ((uint*)push.vertices)[at];
    uint vertex_count = ((uint*)push.vertices)[at + 1];
    uint first_triangle = ((uint*)push.vertices)[at + 2];
    uint triangle_count = ((uint*)push.vertices)[at + 3];
    SetMeshOutputCounts(vertex_count, triangle_count);
    if (thread < vertex_count) {
        uint index = ((uint*)push.vertices)[header.meshlet_vertices + first_vertex + thread];
        vertices[thread] = placed(unpacked(index), entry.x);
    }
    for (uint t = thread; t < triangle_count; t += MAX_VERTICES) {
        uint byte = first_triangle + 3 * t;
        triangles[t] = uint3(triangle_byte(header, byte), triangle_byte(header, byte + 1),
                             triangle_byte(header, byte + 2));
    }
}

// `vertex` as instance `instance` places it in the world and the view sees it, and as its transform
// the frame before placed it.
Varyings placed(Vertex vertex, uint instance) {
    float4x4 transform = push.transforms[instance];
    float4 world = mul(transform, float4(vertex.position, 1.0));
    float4 was = mul(push.transforms[instance + push.previous], float4(vertex.position, 1.0));
    Turned turned = turned(vertex, transform);
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.world = world.xyz;
    out.moved = world.xyz - was.xyz;
    out.uv = vertex.uv;
    out.color = vertex.color;
    out.normal = turned.normal;
    out.tangent = turned.tangent;
    out.side = float2(turned.side, 0.0);
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

// The color, the instance's ID, and how far its point moved in the world since the frame before,
// which the view keeps where what's opaque shows, as games' shaders may return it.
struct Shaded {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
    float4 motion : SV_Target2;
};

[shader("fragment")]
Shaded fragment_main(Varyings input, bool front: SV_IsFrontFace) {
    Frame frame = *push.frame;
    Material material = frame.materials[push.params[input.instance].material];
    // Every texture sampled before any branch or discard, which would leave the derivatives that
    // pick their levels undefined. Base colors decode from sRGB as they're sampled, as the
    // emission's and a specular-glossiness map's specular colors do; the other maps, and that
    // map's glossiness in alpha, are read as stored.
    float4 color = sampled(material, 0, input.uv) * material.color * input.color;
    float4 surface = sampled(material, 1, input.uv);
    float2 mapped = sampled(material, 2, input.uv).xy * 2.0 - 1.0;
    float occlusion = 1.0 + material.occlusion_strength * (sampled(material, 3, input.uv).r - 1.0);
    float3 emission = sampled(material, 4, input.uv).rgb * material.emissive;
#if defined(MASKED)
    if (color.a < material.cutoff) {
        discard;
    }
#elif !defined(OPAQUE)
    if (color.a == 0.0) {
        discard;
    }
#endif
#if defined(OPAQUE) || defined(MASKED)
    color.a = 1.0;
#endif
    Shaded out;
    out.id = input.id;
    // Vulkan's front winds counterclockwise as seen, whichever faces the run draws, which a
    // transform that mirrors shows of its mesh's backs.
    bool shows_front = front != (input.side.x < 0.0);
    float4 reflected = float4(0.0);
#if defined(OPAQUE) || defined(MASKED)
    // The view's ambient occlusion and reflections are of what's opaque, as its depth has it: what
    // blends shows over that, and takes neither.
    uint2 pixel = uint2(input.position.xy);
    occlusion = ambient(frame, pixel, occlusion);
    reflected = reflection(frame, pixel, roughness_of(material, surface));
#endif
    out.color = lit(frame, material, input.world, input.normal, input.tangent, shows_front, color,
                    surface, mapped, occlusion, emission, reflected);
    out.motion = float4(input.moved, 0.0);
#if !defined(OPAQUE) && !defined(MASKED)
    // What blends fogs itself, over what the fog's pass fogged behind it; what's opaque, the fog's
    // pass fogs.
    out.color = fogged(fog_at(frame, input.world), out.color);
#endif
    return out;
}

#if defined(MASKED)
// A masked material's shadow, drawn into a shadow map: none where it's less opaque than its
// cutoff, as it draws nothing there.
[shader("fragment")]
void shadow_main(Varyings input) {
    // Loaded whole, as fragment_main's is. Read for its material table alone, slangc reaches the
    // table's address through a pointer into the frame, even unoptimized, which AMD's driver
    // (25.10.30) miscompiles beside a texture's sample, reading the frame at address 0 and losing
    // the device.
    Frame frame = loadAligned<8>(push.frame);
    Material material = frame.materials[push.params[input.instance].material];
    float alpha = sampled(material, 0, input.uv).a * material.color.a * input.color.a;
    if (alpha < material.cutoff) {
        discard;
    }
}
#endif

// What a fragment drawn into the visibility buffer takes: where its triangle shows, and for a
// masked material, what cuts it out; its instance's ID; and, drawn through vertex shaders, the
// meshlet and triangle it shows, as the visibility buffer holds them.
struct Visible {
    float4 position : SV_Position;
#if defined(MASKED)
    float2 uv;
    float alpha;
    nointerpolation uint instance;
#endif
    nointerpolation uint2 id;
    nointerpolation uint triangle;
};

// `vertex` as instance `instance` places it in the view, as `placed` does, of the meshlet and
// triangle `triangle` says.
Visible visible(Vertex vertex, uint instance, uint triangle) {
    float4x4 transform = push.transforms[instance];
    float4 world = mul(transform, float4(vertex.position, 1.0));
    Visible out;
    out.position = mul(push.clip_from_world, world);
#if defined(MASKED)
    out.uv = vertex.uv;
    out.alpha = vertex.color.a;
    out.instance = instance;
#endif
    out.id = push.ids[instance];
    out.triangle = triangle;
    return out;
}

// A corner of a meshlet a view kept, drawn into the visibility buffer through vertex shaders, as
// `meshlet_vertex_main` draws it, with its meshlet's place among those the draw kept, and its
// triangle's among the meshlet's: each triangle's corners are its own, so they all carry it.
[shader("vertex")]
Visible meshlet_visibility_main(uint corner: SV_VulkanVertexID, uint kept: SV_VulkanInstanceID) {
    uint2 vertex = meshlet_corner(corner, kept);
    return visible(unpacked(vertex.x), vertex.y, kept << 7 | corner / 3);
}

// What a mesh shader writes of each triangle into the visibility buffer: its meshlet's place
// among those the draw kept, and its own among the meshlet's, as its primitive's ID, which the
// fragments behind it read.
struct Primitive {
    uint triangle : SV_PrimitiveID;
};

// A meshlet a view kept, drawn into the visibility buffer through a mesh shader, as `mesh_main`
// draws it.
[shader("mesh")]
[outputtopology("triangle")]
[numthreads(MAX_VERTICES, 1, 1)]
void mesh_visibility_main(uint3 group: SV_GroupID, uint thread: SV_GroupIndex,
                          OutputVertices<Visible, MAX_VERTICES> vertices,
                          OutputIndices<uint3, MAX_TRIANGLES> triangles,
                          OutputPrimitives<Primitive, MAX_TRIANGLES> primitives) {
    uint kept = group.y * ACROSS + group.x;
    if (kept >= push.meshlets[0].y) {
        SetMeshOutputCounts(0, 0);
        return;
    }
    uint2 entry = push.meshlets[KEPT + kept];
    Header header = *(Header*)push.vertices;
    uint at = header.meshlets + entry.y * 4;
    uint first_vertex = ((uint*)push.vertices)[at];
    uint vertex_count = ((uint*)push.vertices)[at + 1];
    uint first_triangle = ((uint*)push.vertices)[at + 2];
    uint triangle_count = ((uint*)push.vertices)[at + 3];
    SetMeshOutputCounts(vertex_count, triangle_count);
    if (thread < vertex_count) {
        uint index = ((uint*)push.vertices)[header.meshlet_vertices + first_vertex + thread];
        vertices[thread] = visible(unpacked(index), entry.x, 0);
    }
    for (uint t = thread; t < triangle_count; t += MAX_VERTICES) {
        uint byte = first_triangle + 3 * t;
        triangles[t] = uint3(triangle_byte(header, byte), triangle_byte(header, byte + 1),
                             triangle_byte(header, byte + 2));
        Primitive primitive;
        primitive.triangle = kept << 7 | t;
        primitives[t] = primitive;
    }
}

// What a fragment writes into the visibility buffer, and its instance's ID.
struct Seen {
    // Which draw drew it, `push.draw`, and the meshlet and triangle it shows.
    uint2 visible : SV_Target0;
    uint2 id : SV_Target1;
};

// What a fragment of the triangle `triangle` says writes into the visibility buffer: none for a
// masked material where it's less opaque than its cutoff, as `fragment_main` draws none.
Seen seen(Visible input, uint triangle) {
#if defined(MASKED)
    // Loaded whole, as `shadow_main` says why.
    Frame frame = loadAligned<8>(push.frame);
    Material material = frame.materials[push.params[input.instance].material];
    float alpha = sampled(material, 0, input.uv).a * material.color.a * input.alpha;
    if (alpha < material.cutoff) {
        discard;
    }
#endif
    Seen out;
    out.visible = uint2(push.draw, triangle);
    out.id = input.id;
    return out;
}

// A fragment drawn into the visibility buffer through vertex shaders, whose corners carry their
// triangle.
[shader("fragment")]
Seen visibility_main(Visible input) {
    return seen(input, input.triangle);
}

// A fragment drawn into the visibility buffer behind a mesh shader, which writes its triangle as
// its primitive's ID. A module of its own, as reading that takes the Geometry capability, which
// only devices that draw through mesh shaders enable.
[shader("fragment")]
Seen visibility_primitive_main(Visible input, uint triangle: SV_PrimitiveID) {
    return seen(input, triangle);
}
