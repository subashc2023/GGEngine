// Traces paths through the view's opaque and masked models, as src/trace.rs plans it: a reference
// for the light the view shades. Each pixel, a thread, traces `Trace::samples` paths from the
// camera, adds what they bring to the light its earlier paths brought, unexposed, and writes the
// mean, exposed, into the view's colors for the tonemap pass.
//
// Unidirectional path tracing, at most `MOST_BOUNCES` bounces, Russian roulette after
// `SURE_BOUNCES`:
// - At each hit, every light by next-event estimation, with a shadow ray: the frame's lights are
//   deltas, and the sky holds no sun a directional light stands for.
// - Then a bounce by the BRDF's sampling: GGX's visible normals (Dupuy and Benyoub, "Sampling
//   Visible GGX Normals with Spherical Caps", 2023) or the cosine, picked by their estimated shares.
// - The sky's light (its specular cube's level 0) where a path escapes, emission where it hits it,
//   and an unlit material's color, each over the exposure, as the view shows them already exposed.
// - Materials as shading.slang's `surface_of` reads them, their maps at level 0, by its `brdf`,
//   with Turquin's energy compensation ("Practical multiple scattering compensation for
//   microfacet models", 2019) for the light GGX's single scattering loses.
// - The first hit antialiased by jittering each path within its pixel.
// - Back faces culled but where a material is double-sided, as the view's draws cull them.
// - A masked material's triangles hit where its alpha, at level 0, is at least its cutoff, as the
//   masked shader draws them.
//
// `Traced`, `Trace` and `TracePush` match `Traced`, `TraceData` and `TracePush` in src/trace.rs
// field for field.

#include "shading.slang"

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// What hits on an instance read, by its custom index.
struct Traced {
    // Its mesh's buffer, its header first, and its level 0's first index: addresses, as two words
    // each, as visibility.slang's runs hold them.
    uint2 vertices;
    uint2 indices;
    // Its material's index.
    uint material;
    uint unused;
};

// The view, and how many paths its pixels have taken.
struct Trace {
    // From the view's clip space, unjittered, to the world.
    float4x4 world_from_clip;
    // The TLAS's address, as two words.
    uint2 tlas;
    // The view's size in pixels.
    uint2 size;
    // The paths each pixel took in earlier frames, and how many more it takes in this one.
    uint taken;
    uint samples;
    // The view's colors, a bindless storage image.
    uint target;
    uint unused;
};

struct TracePush {
    Trace* trace;
    Frame* frame;
    Traced* traced;
    // Each pixel's light, summed over its paths, unexposed: RGB, by `y * width + x`.
    float4* sums;
};
[[vk::push_constant]] ConstantBuffer<TracePush> push;

// How many pixels across and down a workgroup takes: `GROUP` in src/trace.rs.
static const uint GROUP = 8;
static const uint MOST_BOUNCES = 8;
static const uint SURE_BOUNCES = 3;

// PCG's 32-bit generator (O'Neill, 2014), its state seeded by `pcg2d` from the pixel and the path.
struct Random {
    uint state;
};

Random seeded(uint2 pixel, uint path) {
    Random random;
    random.state = pcg2d(uint2(pixel.x | (pixel.y << 16), path)).x;
    return random;
}

// The next of `random`'s numbers, from 0 to below 1.
float next(inout Random random) {
    random.state = random.state * 747796405u + 2891336453u;
    uint word = ((random.state >> ((random.state >> 28u) + 4u)) ^ random.state) * 277803737u;
    word = (word >> 22u) ^ word;
    return float(word >> 8) * (1.0 / 16777216.0);
}

// Where a ray leaving `p` on a surface of unit normal `n`, toward `n`'s side, starts so that it
// doesn't find that surface again: moved along `n` by a few units of `p`'s last place, or by a
// fixed amount near the origin (Wächter and Binder, "A Fast and Robust Method for Avoiding
// Self-Intersection", Ray Tracing Gems, 2019).
float3 offset_ray(float3 p, float3 n) {
    int3 moved = int3(256.0 * n);
    float3 stepped = asfloat(asint(p) + select(p < 0.0, -moved, moved));
    return select(abs(p) < 1.0 / 32.0, p + n / 65536.0, stepped);
}

// What a ray found: its instance's custom index, its triangle and the weights of its second and
// third corners there, and the instance's transform.
struct Hit {
    bool found;
    uint instance;
    uint primitive;
    float2 weights;
    float3x4 object_to_world;
};

// `material`'s map `which` at `uv`, level 0.
float4 sampled_at(Material material, uint which, float2 uv) {
    return sampled_by(material, which, uv, float2(0.0), float2(0.0));
}

// Whether candidate `primitive` of the instance of custom index `instance`, a masked material's
// triangle, covers where its second and third corners weigh `weights`: where its alpha, its color
// map's at level 0 times its factor's and its vertices', is at least its cutoff.
bool covers(Material* materials, uint instance, uint primitive, float2 weights) {
    Traced traced = push.traced[instance];
    uint* words = bit_cast<uint*>(traced.vertices);
    Header header = *(Header*)words;
    uint* indices = bit_cast<uint*>(traced.indices);
    uint first = 3 * primitive;
    uint3 index = uint3(indices[first], indices[first + 1], indices[first + 2]);
    Vertex a = unpack(header, ((PackedVertex*)words)[HEADER + index.x]);
    Vertex b = unpack(header, ((PackedVertex*)words)[HEADER + index.y]);
    Vertex c = unpack(header, ((PackedVertex*)words)[HEADER + index.z]);
    float3 w = float3(1.0 - weights.x - weights.y, weights);
    float2 uv = w.x * a.uv + w.y * b.uv + w.z * c.uv;
    float alpha = w.x * a.color.a + w.y * b.color.a + w.z * c.color.a;
    Material material = materials[traced.material];
    alpha *= sampled_at(material, 0, uv).a * material.color.a;
    return alpha >= material.cutoff;
}

// Commits `query`'s candidates that cover, as `covers` says, until it ends.
void proceed<let Flags : uint>(inout RayQuery<Flags> query, Material* materials) {
    while (query.Proceed()) {
        if (query.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE
            && covers(materials, query.CandidateInstanceID(), query.CandidatePrimitiveIndex(),
                      query.CandidateTriangleBarycentrics())) {
            query.CommitNonOpaqueTriangleHit();
        }
    }
}

// The nearest front face `direction` finds from `origin`, or a double-sided material's back face.
Hit nearest(RaytracingAccelerationStructure tlas, Material* materials, float3 origin,
            float3 direction) {
    RayDesc ray;
    ray.Origin = origin;
    ray.Direction = direction;
    ray.TMin = 0.0;
    ray.TMax = FARTHEST;
    RayQuery<RAY_FLAG_CULL_BACK_FACING_TRIANGLES> query;
    query.TraceRayInline(tlas, RAY_FLAG_NONE, 0xFF, ray);
    proceed(query, materials);
    Hit hit;
    hit.found = query.CommittedStatus() == COMMITTED_TRIANGLE_HIT;
    hit.instance = query.CommittedInstanceID();
    hit.primitive = query.CommittedPrimitiveIndex();
    hit.weights = query.CommittedTriangleBarycentrics();
    hit.object_to_world = query.CommittedObjectToWorld3x4();
    return hit;
}

// Whether nothing is within `reach` of `origin` along `direction`.
bool unoccluded(RaytracingAccelerationStructure tlas, Material* materials, float3 origin,
                float3 direction, float reach) {
    RayDesc ray;
    ray.Origin = origin;
    ray.Direction = direction;
    ray.TMin = 0.0;
    ray.TMax = reach;
    RayQuery<RAY_FLAG_CULL_BACK_FACING_TRIANGLES | RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH> query;
    query.TraceRayInline(tlas, RAY_FLAG_NONE, 0xFF, ray);
    proceed(query, materials);
    return query.CommittedStatus() == COMMITTED_NOTHING;
}

// What a hit found of its mesh, in the world: its material, the point, its interpolated normal and
// tangent as shading takes them, its UV and vertices' color, the triangle's unit normal on the side
// its mesh winds counterclockwise, and whether the ray came from that side.
struct Found {
    Material material;
    float3 world;
    float3 normal;
    float4 tangent;
    float2 uv;
    float4 color;
    float3 geometric;
    bool front;
};

Found found(Material* materials, Hit hit, float3 direction) {
    Traced traced = push.traced[hit.instance];
    uint* words = bit_cast<uint*>(traced.vertices);
    Header header = *(Header*)words;
    uint* indices = bit_cast<uint*>(traced.indices);
    uint first = 3 * hit.primitive;
    uint3 index = uint3(indices[first], indices[first + 1], indices[first + 2]);
    Vertex a = unpack(header, ((PackedVertex*)words)[HEADER + index.x]);
    Vertex b = unpack(header, ((PackedVertex*)words)[HEADER + index.y]);
    Vertex c = unpack(header, ((PackedVertex*)words)[HEADER + index.z]);
    float4x4 transform = float4x4(hit.object_to_world[0], hit.object_to_world[1],
                                  hit.object_to_world[2], float4(0.0, 0.0, 0.0, 1.0));
    Turned ta = turned(a, transform);
    Turned tb = turned(b, transform);
    Turned tc = turned(c, transform);
    float3 w = float3(1.0 - hit.weights.x - hit.weights.y, hit.weights);
    float3 wa = mul(transform, float4(a.position, 1.0)).xyz;
    float3 wb = mul(transform, float4(b.position, 1.0)).xyz;
    float3 wc = mul(transform, float4(c.position, 1.0)).xyz;
    Found out;
    out.material = materials[traced.material];
    out.world = w.x * wa + w.y * wb + w.z * wc;
    out.normal = w.x * ta.normal + w.y * tb.normal + w.z * tc.normal;
    out.tangent = w.x * ta.tangent + w.y * tb.tangent + w.z * tc.tangent;
    out.uv = w.x * a.uv + w.y * b.uv + w.z * c.uv;
    out.color = w.x * a.color + w.y * b.color + w.z * c.color;
    // A mirroring transform reverses the winding the cross product follows.
    out.geometric = normalize(cross(wb - wa, wc - wa)) * ta.side;
    out.front = dot(out.geometric, direction) < 0.0;
    return out;
}

// An orthonormal basis about unit `n` (Duff et al., "Building an Orthonormal Basis, Revisited",
// 2017).
void basis(float3 n, out float3 t, out float3 b) {
    float s = n.z >= 0.0 ? 1.0 : -1.0;
    float a = -1.0 / (s + n.z);
    float c = n.x * n.y * a;
    t = float3(1.0 + s * n.x * n.x * a, s * c, -s * n.x);
    b = float3(c, s + n.y * n.y * a, -n.y);
}

// A normal of GGX's of `alpha` visible from `v`, about +Z, from two uniform numbers `u`.
float3 visible_normal(float3 v, float alpha, float2 u) {
    float3 stretched = normalize(float3(alpha * v.x, alpha * v.y, v.z));
    float phi = 2.0 * PI * u.x;
    float z = (1.0 - u.y) * (1.0 + stretched.z) - stretched.z;
    float sine = sqrt(saturate(1.0 - z * z));
    float3 h = float3(sine * cos(phi), sine * sin(phi), z) + stretched;
    return normalize(float3(alpha * h.x, alpha * h.y, max(h.z, 0.0)));
}

// The density of `l` sampled by `visible_normal`'s reflection of `v`, about +Z: the visible
// normals' over 4 v·h, which is G1(v) D(h) over 4 n·v.
float specular_density(float3 v, float3 l, float alpha2) {
    float3 h = normalize(v + l);
    float spread = h.z * h.z * (alpha2 - 1.0) + 1.0;
    float distribution = alpha2 / (PI * spread * spread);
    float masking = 2.0 * v.z / (v.z + sqrt(alpha2 + (1.0 - alpha2) * v.z * v.z));
    return masking * distribution / (4.0 * v.z);
}

// The light one path from `origin` along `direction` brings, unexposed.
float3 path(Frame frame, RaytracingAccelerationStructure tlas, float3 origin, float3 direction,
            inout Random random) {
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    float3 light = float3(0.0);
    float3 throughput = float3(1.0);
    for (uint bounce = 0; bounce < MOST_BOUNCES; bounce++) {
        Hit hit = nearest(tlas, frame.materials, origin, direction);
        if (!hit.found) {
            if (frame.specular_levels > 0) {
                float3 sky = cubes[frame.specular].SampleLevel(clamp_, direction, 0.0).rgb;
                light += throughput * sky * frame.sky_luminance;
            }
            break;
        }
        Found at = found(frame.materials, hit, direction);
        Material material = at.material;
        float4 color = sampled_at(material, 0, at.uv) * material.color * at.color;
        if ((material.flags & UNLIT) != 0) {
            light += throughput * color.rgb / frame.exposure;
            break;
        }
        float4 surface = sampled_at(material, 1, at.uv);
        float2 mapped = sampled_at(material, 2, at.uv).xy * 2.0 - 1.0;
        float3 emission = sampled_at(material, 4, at.uv).rgb * material.emissive;
        light += throughput * emission / frame.exposure;
        float roughness = roughness_of(material, surface);
        float metallic;
        Surface lit = surface_of(material, at.world, -direction, at.normal, at.tangent, at.front,
                                 color.rgb, surface, roughness, mapped, metallic);
        // Single scattering's albedo at F0 of 1, by the split sum's table, which a frame has with
        // a sky alone; without one, uncompensated.
        if (frame.specular_levels > 0) {
            float2 table =
                textures[frame.brdf].SampleLevel(clamp_, float2(lit.n_dot_v, roughness), 0.0).rg;
            lit.specular_weight = 1.0 + lit.f0 * (1.0 / (table.x + table.y) - 1.0);
        }
        // Facing the side the ray came from.
        float3 geometric = at.front ? at.geometric : -at.geometric;
        float3 from = offset_ray(at.world, geometric);
        for (uint i = 0; i < frame.light_count; i++) {
            float3 to_light;
            float3 arrived = arriving(frame, i, at.world, to_light);
            float n_dot_l = dot(lit.normal, to_light);
            if (n_dot_l <= 0.0 || dot(geometric, to_light) <= 0.0) {
                continue;
            }
            Light shining = frame.lights[i];
            float reach = shining.kind == 0 ? FARTHEST : length(shining.position - from);
            if (unoccluded(tlas, frame.materials, from, to_light, reach)) {
                light += throughput * brdf(lit, to_light, n_dot_l) * arrived * n_dot_l;
            }
        }
        // The next way, about the shading normal.
        float3 t;
        float3 b;
        basis(lit.normal, t, b);
        float3 view = -direction;
        float3 v = float3(dot(view, t), dot(view, b), lit.n_dot_v);
        v = normalize(v);
        float3 fresnel = lit.f0 + (1.0 - lit.f0) * pow5(1.0 - lit.n_dot_v);
        float specular_share = luminance(fresnel * lit.specular_weight);
        float diffuse_share = luminance(lit.diffuse_color * (1.0 - fresnel));
        if (specular_share + diffuse_share <= 0.0) {
            break;
        }
        float specular_chance = specular_share / (specular_share + diffuse_share);
        float2 u = float2(next(random), next(random));
        float3 l;
        if (next(random) < specular_chance) {
            l = reflect(-v, visible_normal(v, sqrt(lit.alpha2), u));
        } else {
            float r = sqrt(u.x);
            float phi = 2.0 * PI * u.y;
            l = float3(r * cos(phi), r * sin(phi), sqrt(1.0 - u.x));
        }
        float3 out_way = t * l.x + b * l.y + lit.normal * l.z;
        if (l.z <= 0.0 || dot(out_way, geometric) <= 0.0) {
            break;
        }
        float density = specular_chance * specular_density(v, l, lit.alpha2)
            + (1.0 - specular_chance) * l.z / PI;
        throughput *= brdf(lit, out_way, l.z) * (l.z / density);
        if (bounce + 1 >= SURE_BOUNCES) {
            float kept = min(max(max(throughput.r, throughput.g), throughput.b), 0.95);
            if (next(random) >= kept) {
                break;
            }
            throughput /= kept;
        }
        origin = from;
        direction = out_way;
    }
    return light;
}

[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void trace_main(uint3 pixel: SV_DispatchThreadID) {
    Trace trace = *push.trace;
    if (pixel.x >= trace.size.x || pixel.y >= trace.size.y) {
        return;
    }
    // Loaded whole, as model.slang's `shadow_main` says why.
    Frame frame = loadAligned<8>(push.frame);
    RaytracingAccelerationStructure tlas =
        RaytracingAccelerationStructure(bit_cast<uint64_t>(trace.tlas));
    uint index = pixel.y * trace.size.x + pixel.x;
    float3 sum = trace.taken > 0 ? push.sums[index].rgb : float3(0.0);
    for (uint s = 0; s < trace.samples; s++) {
        Random random = seeded(pixel.xy, trace.taken + s);
        float2 at = (float2(pixel.xy) + float2(next(random), next(random))) / float2(trace.size);
        float4 clip = float4(at * 2.0 - 1.0, 1.0, 1.0);
        // Reversed depth: the near plane at 1, the far one (infinite for a perspective) at 0.
        float4 near = mul(trace.world_from_clip, clip);
        float4 far = mul(trace.world_from_clip, float4(clip.xy, 0.0, 1.0));
        float3 origin = near.xyz / near.w;
        float3 direction = normalize(far.xyz - origin * far.w);
        sum += path(frame, tlas, origin, direction, random);
    }
    push.sums[index] = float4(sum, 0.0);
    float3 mean = sum / float(trace.taken + trace.samples);
    storage[trace.target][uint3(pixel.xy, 0)] = float4(min(mean * frame.exposure, BRIGHTEST), 1.0);
}
