// Models' meshes, shaded as shading.slang says, each instance with its own transform, ID and
// material. It writes its ID, and its motion in the world since the frame before, for
// antialiasing. `Params` matches `Params` in src/model.rs byte for byte.
//
// As is, it's for blended materials: it discards what's fully transparent, for picks, and writes
// no depth, keeping early depth testing. model_opaque.slang and model_masked.slang include it with
// OPAQUE or MASKED defined, for materials drawn fully opaque, alpha 1:
// - Only MASKED discards, below the cutoff: discarding while writing depth loses early depth
//   testing on some GPUs. Its shadows are cut out by `shadow_main`; opaque ones draw whole,
//   without a fragment shader.
// - Both draw normals by `normal_main` for the view's reflections, where they aren't drawn into
//   the visibility buffer.
//
// What blends fogs itself (fog.slang); the fog's pass fogs what's opaque.
//
// Kept meshlets also draw into the visibility buffer, which visibility.slang shades: each fragment
// writes its draw, meshlet, triangle and ID, cut out as `fragment_main` cuts it.

#include "shading.slang"
#include "fog.slang"

struct Params {
    // The material's index in the frame's table.
    uint material;
};

[[vk::push_constant]] ConstantBuffer<Push<PackedVertex, Params>> push;

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
    // In x, -1 where the instance's transform mirrors it, else 1. A vector: Debian 13's validation
    // layer (1.4.309) warns of a scalar output the next stage doesn't read, as `shadow_main`
    // doesn't.
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
// Workgroups across a mesh shader's draw of meshlets: `ACROSS` in meshlets.slang.
static const uint ACROSS = 32768;

// Byte `byte` of the mesh's meshlets' triangles.
uint triangle_byte(Header header, uint byte) {
    uint word = ((uint*)push.vertices)[header.triangles + byte / 4];
    return (word >> ((byte & 3) * 8)) & 0xFF;
}

// The mesh's index of the vertex at `corner` of the draw's `kept`th meshlet, drawn through vertex
// shaders, and its instance's place among the draw's.
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

// A kept meshlet's corner through vertex shaders, each meshlet an instance of `MAX_TRIANGLES`
// triangles. Those past its own are degenerate, every corner its first vertex.
[shader("vertex")]
Varyings meshlet_vertex_main(uint corner: SV_VulkanVertexID, uint kept: SV_VulkanInstanceID) {
    uint2 vertex = meshlet_corner(corner, kept);
    return placed(unpacked(vertex.x), vertex.y);
}

// A kept meshlet, through a mesh shader: a workgroup each, `ACROSS` across and as many rows as they
// take, those past the draw's meshlets drawing nothing. A thread per vertex, and per triangle and
// the one `MAX_VERTICES` after it.
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

// `vertex` placed by `instance` in the world and the view, and by its transform the frame before.
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

// The color, the instance's ID, and its motion in the world since the frame before, which the view
// keeps where what's opaque shows, as games' shaders may return it.
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
    // pick their levels undefined. Base, emission and specular-glossiness specular colors decode
    // from sRGB as sampled; other maps, and glossiness in alpha, read as stored.
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
    // Vulkan's front winds counterclockwise as seen, whichever faces the run draws; a mirroring
    // transform shows its mesh's backs so.
    bool shows_front = front != (input.side.x < 0.0);
    float4 reflected = float4(0.0);
#if defined(OPAQUE) || defined(MASKED)
    // The view's ambient occlusion and reflections are of the opaque depth; what blends over it
    // takes neither.
    uint2 pixel = uint2(input.position.xy);
    occlusion = ambient(frame, pixel, occlusion);
    reflected = reflection(frame, pixel, roughness_of(material, surface));
#endif
    out.color = lit(frame, material, input.world, input.normal, input.tangent, shows_front, color,
                    surface, mapped, occlusion, emission, reflected);
    out.motion = float4(input.moved, 0.0);
#if !defined(OPAQUE) && !defined(MASKED)
    out.color = fogged(fog_at(frame, input.world), out.color);
#endif
    return out;
}

#if defined(MASKED)
// A masked material's shadow: none below its cutoff, where it draws nothing.
[shader("fragment")]
void shadow_main(Varyings input) {
    // Loaded whole, as fragment_main's is. Read for its material table alone, slangc reaches the
    // table's address through a pointer into the frame, even unoptimized, which AMD's driver
    // (25.10.30) miscompiles beside a texture sample: it reads the frame at address 0 and loses
    // the device.
    Frame frame = loadAligned<8>(push.frame);
    Material material = frame.materials[push.params[input.instance].material];
    float alpha = sampled(material, 0, input.uv).a * material.color.a * input.color.a;
    if (alpha < material.cutoff) {
        discard;
    }
}
#endif

#if defined(OPAQUE) || defined(MASKED)
// The world normal of what's opaque, as `fragment_main` shades it, and 1 in alpha, for the view's
// reflections' rays. Drawn against the finished depth, where nearest, before anything shades; none
// below a masked material's cutoff.
[shader("fragment")]
float4 normal_main(Varyings input, bool front: SV_IsFrontFace) : SV_Target0 {
    // Loaded whole, as `shadow_main` says why.
    Frame frame = loadAligned<8>(push.frame);
    Material material = frame.materials[push.params[input.instance].material];
    // Sampled before the discard, as `fragment_main` says why.
    float2 mapped = sampled(material, 2, input.uv).xy * 2.0 - 1.0;
#if defined(MASKED)
    float alpha = sampled(material, 0, input.uv).a * material.color.a * input.color.a;
    if (alpha < material.cutoff) {
        discard;
    }
#endif
    bool shows_front = front != (input.side.x < 0.0);
    float3 surface_normal;
    return float4(shading_normal(material, input.normal, input.tangent, shows_front, mapped,
                                 surface_normal), 1.0);
}
#endif

// What a fragment drawn into the visibility buffer takes; `triangle`, through vertex shaders, its
// meshlet and triangle.
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

// `vertex` placed as `placed` does, of the meshlet and triangle `triangle` says.
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

// `meshlet_vertex_main` into the visibility buffer, carrying its meshlet's place in the draw and
// its triangle's in the meshlet: no corner is shared, so all carry it.
[shader("vertex")]
Visible meshlet_visibility_main(uint corner: SV_VulkanVertexID, uint kept: SV_VulkanInstanceID) {
    uint2 vertex = meshlet_corner(corner, kept);
    return visible(unpacked(vertex.x), vertex.y, kept << 7 | corner / 3);
}

// Each triangle's meshlet's place in the draw and its own in the meshlet, as its primitive ID,
// which its fragments read.
struct Primitive {
    uint triangle : SV_PrimitiveID;
};

// `mesh_main` into the visibility buffer.
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

// What a fragment of `triangle` writes: none below a masked material's cutoff.
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

// Through vertex shaders, whose corners carry their triangle.
[shader("fragment")]
Seen visibility_main(Visible input) {
    return seen(input, input.triangle);
}

// Behind a mesh shader, which writes the triangle as the primitive ID. A module of its own:
// reading it takes the Geometry capability, which only devices drawing through mesh shaders enable.
[shader("fragment")]
Seen visibility_primitive_main(Visible input, uint triangle: SV_PrimitiveID) {
    return seen(input, triangle);
}
