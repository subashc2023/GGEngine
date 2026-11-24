// Traces paths through the view's opaque and masked models, as src/trace.rs plans it: a reference
// for the light the view shades. Each pixel, a thread, traces `Trace::samples` paths from the
// camera, adds what they bring to the light its earlier paths brought, unexposed, and writes the
// mean, exposed, into the view's colors for the tonemap pass.
//
// Unidirectional path tracing, at most `MOST_BOUNCES` bounces, Russian roulette after
// `SURE_BOUNCES`:
// - At each hit, every light whose light reaches it by next-event estimation, with a shadow ray:
//   the frame's lights are deltas, and the sky holds no sun a directional light stands for.
// - Then a bounce: the walk's way out (below), or the cosine, picked by their estimated shares.
// - The sky's light (its specular cube's level 0) where a path escapes, emission where it hits it,
//   and an unlit material's color, each over the exposure, as the view shows them already exposed.
// - Emission and unlit colors only on paths whose every bounce took the specular lobe, as the
//   raster shows them: directly and in reflections, lighting nothing; lights stand for what glows.
// - Materials as shading.slang's `surface_of` reads them, their maps at level 0, by its `brdf`.
//   Specular light scattered more than once, which `brdf`'s GGX loses, by Heitz et al.'s random
//   walk on the microsurface (`walked`): exact, where compensations such as Turquin's and the
//   raster's Fdez-Agüera's are 10% off for a rough colored metal.
// - After a bounce, surfaces at least as rough as the roughest the path has bounced off, a diffuse
//   bounce the roughest, up to `MOST_ROUGHNESS_FLOOR`: path space regularization (Kaplanyan and
//   Dachsbacher, 2013). A rough bounce rarely finds a smooth surface's peak toward a light, so a
//   few paths bring all its light: specks. It blurs that light, which the raster's diffuse light
//   leaves out and its reflections show only as the view sees it.
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
// Roulette ends nearly every path before it: at 8, a closed room of albedo 0.8 loses 0.8^8 of its
// light, 17%.
static const uint MOST_BOUNCES = 64;
static const uint SURE_BOUNCES = 3;
// The most the roughness floor rises to. At 0.4, Bistro keeps half again as many specks by its
// manhole; at 1, it loses nearly twice the light there, for a tenth fewer. Cycles' Filter Glossy,
// at its default, floors GGX's alpha after a diffuse bounce at 0.41 or more; this, at 0.25.
static const float MOST_ROUGHNESS_FLOOR = 0.5;

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
    Hit hit = {};
    hit.found = query.CommittedStatus() == COMMITTED_TRIANGLE_HIT;
    // Only of a hit: a miss's are undefined, and lavapipe reads them through a null instance.
    if (hit.found) {
        hit.instance = query.CommittedInstanceID();
        hit.primitive = query.CommittedPrimitiveIndex();
        hit.weights = query.CommittedTriangleBarycentrics();
        hit.object_to_world = query.CommittedObjectToWorld3x4();
    }
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
    // The instance's transform takes the counts its BLAS holds, read as UNORM (trace.rs); this
    // one takes the mesh's positions, `offset + steps * step`.
    float scale = header.step * 65535.0;
    float3 moved = -header.offset / scale;
    float4x4 counts_from_mesh = float4x4(
        1.0 / scale, 0.0, 0.0, moved.x,
        0.0, 1.0 / scale, 0.0, moved.y,
        0.0, 0.0, 1.0 / scale, moved.z,
        0.0, 0.0, 0.0, 1.0);
    float4x4 transform = mul(float4x4(hit.object_to_world[0], hit.object_to_world[1],
                                      hit.object_to_world[2], float4(0.0, 0.0, 0.0, 1.0)),
                             counts_from_mesh);
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

// A normal of GGX's of `alpha` visible from `v`, about +Z, from two uniform numbers `u` (Dupuy and
// Benyoub, "Sampling Visible GGX Normals with Spherical Caps", 2023). `v` may be below the horizon,
// as a walk's are: its density, v·h over π on the sphere, holds for any `v`.
float3 visible_normal(float3 v, float alpha, float2 u) {
    float3 stretched = normalize(float3(alpha * v.x, alpha * v.y, v.z));
    float phi = 2.0 * PI * u.x;
    float z = (1.0 - u.y) * (1.0 + stretched.z) - stretched.z;
    float sine = sqrt(saturate(1.0 - z * z));
    float3 h = float3(sine * cos(phi), sine * sin(phi), z) + stretched;
    return normalize(float3(alpha * h.x, alpha * h.y, max(h.z, 0.0)));
}

// Schlick's Fresnel of head-on color `f0` at cosine `c` to the normal.
float3 schlick(float3 f0, float c) {
    return f0 + (1.0 - f0) * pow5(1.0 - saturate(c));
}

// Specular light on a GGX microsurface by Heitz et al.'s random walk ("Multiple-Scattering
// Microfacet BSDFs with the Smith Model", 2016, its conductor): heights uniform from -1 to 1,
// directions about +Z. A walk from the top meets the surface where Smith's masking says, reflects
// off a normal visible from where it came, by Schlick's Fresnel, and leaves or meets it again. Its
// first scattering is `brdf`'s specular.

// The most scatterings a walk takes: rough GGX's need more than 8 for 0.02% of their light, 16 for
// none of a million paths.
static const uint MOST_SCATTERINGS = 16;
// The height of a walk that left: above every height of the surface.
static const float LEFT = 2.0;

// Smith's Λ of GGX of `alpha2` toward unit `w`, either side of the horizon: Λ(-w) = -1 - Λ(w).
float smith_lambda(float3 w, float alpha2) {
    float z2 = w.z * w.z;
    float root = sqrt(1.0 + alpha2 * (1.0 - z2) / z2);
    return w.z > 0.0 ? 0.5 * (root - 1.0) : -0.5 * (root + 1.0);
}

// The share of the surface's heights below `h`.
float below(float h) {
    return saturate(0.5 * (h + 1.0));
}

// Where a walk going `w` from height `h` next meets the surface, or `LEFT`, from uniform `u`. Going
// up, it leaves with the chance nothing above masks it, Smith's G1 at `h`: below(h)^Λ(w).
float next_height(float3 w, float h, float alpha2, float u) {
    if (w.z > 0.9999) {
        return LEFT;
    }
    if (w.z < -0.9999) {
        return 2.0 * u * below(h) - 1.0;
    }
    if (abs(w.z) < 0.0001) {
        return h;
    }
    float lambda = smith_lambda(w, alpha2);
    if (w.z > 0.0 && u > 1.0 - pow(below(h), lambda)) {
        return LEFT;
    }
    return clamp(2.0 * below(h) / pow(1.0 - u, 1.0 / lambda) - 1.0, -1.0, 1.0);
}

// A way `l` out of the surface from view `v` by one walk, and what of the light arriving along it
// reaches `v`, its scatterings' Fresnel: the BRDF times the cosine over the density of `l`. False
// where the walk hasn't left after `MOST_SCATTERINGS`.
bool walked(float3 v, float alpha2, float3 f0, inout Random random, out float3 l,
            out float3 carried) {
    float alpha = sqrt(alpha2);
    l = -v;
    carried = float3(1.0);
    float h = 1.0;
    for (uint i = 0; i <= MOST_SCATTERINGS; i++) {
        h = next_height(l, h, alpha2, next(random));
        if (h == LEFT) {
            return true;
        }
        if (i == MOST_SCATTERINGS) {
            break;
        }
        float3 from = -l;
        float3 m = visible_normal(from, alpha, float2(next(random), next(random)));
        carried *= schlick(f0, dot(from, m));
        l = reflect(l, m);
    }
    return false;
}

// The light from `l` that one walk from view `v` reflects toward `v` after its first scattering,
// per unit of that light: the BRDF past `brdf`'s specular, times the cosine, as an estimate whose
// mean is exact. At each scattering, the share its normals visible from where it came reflect
// toward `l`, times the chance nothing above masks `l`.
float3 scattered_later(float3 v, float3 l, float alpha2, float3 f0, inout Random random) {
    float alpha = sqrt(alpha2);
    float lambda = smith_lambda(l, alpha2);
    float3 way = -v;
    float3 carried = float3(1.0);
    float3 sum = float3(0.0);
    float h = 1.0;
    for (uint i = 0; i < MOST_SCATTERINGS; i++) {
        h = next_height(way, h, alpha2, next(random));
        if (h == LEFT) {
            break;
        }
        float3 from = -way;
        float3 m = normalize(from + l);
        float c = dot(from, m);
        if (i > 0 && m.z > 0.0 && c > 0.0) {
            float spread = m.z * m.z * (alpha2 - 1.0) + 1.0;
            float distribution = alpha2 / (PI * spread * spread);
            // The surface's area seen from `from`, by which its visible normals' density divides.
            float seen = 0.5 * (from.z + sqrt(from.z * from.z + (1.0 - from.z * from.z) * alpha2));
            float unmasked = l.z > 0.9999 ? 1.0 : pow(below(h), lambda);
            sum += carried * schlick(f0, c) * (distribution / (4.0 * seen) * unmasked);
        }
        float3 next_normal = visible_normal(from, alpha, float2(next(random), next(random)));
        carried *= schlick(f0, dot(from, next_normal));
        way = reflect(way, next_normal);
    }
    return sum;
}

// The light one path from `origin` along `direction` brings, unexposed.
float3 path(Frame frame, RaytracingAccelerationStructure tlas, float3 origin, float3 direction,
            inout Random random) {
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    float3 light = float3(0.0);
    float3 throughput = float3(1.0);
    // Whether every bounce so far took the specular lobe.
    bool shown = true;
    // How rough surfaces are at least: the roughest the path has bounced off, a diffuse bounce the
    // roughest, up to `MOST_ROUGHNESS_FLOOR`. It never falls, as each surface is at least it.
    float least_roughness = 0.0;
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
            if (shown) {
                light += throughput * color.rgb / frame.exposure;
            }
            break;
        }
        float4 surface = sampled_at(material, 1, at.uv);
        float2 mapped = sampled_at(material, 2, at.uv).xy * 2.0 - 1.0;
        if (shown) {
            float3 emission = sampled_at(material, 4, at.uv).rgb * material.emissive;
            light += throughput * emission / frame.exposure;
        }
        float roughness = max(roughness_of(material, surface), least_roughness);
        float metallic;
        Surface lit = surface_of(material, at.world, -direction, at.normal, at.tangent, at.front,
                                 color.rgb, surface, roughness, mapped, metallic);
        // Facing the side the ray came from.
        float3 geometric = at.front ? at.geometric : -at.geometric;
        float3 from = offset_ray(at.world, geometric);
        // Ways about the shading normal, for the walk.
        float3 t;
        float3 b;
        basis(lit.normal, t, b);
        float3 view = -direction;
        float3 v = normalize(float3(dot(view, t), dot(view, b), lit.n_dot_v));
        for (uint i = 0; i < frame.light_count; i++) {
            float3 to_light;
            float3 arrived = arriving(frame.lights, i, at.world, to_light);
            float n_dot_l = dot(lit.normal, to_light);
            // Lights out of range or cone skipped: under Bistro's 96 point lights, their shadow
            // rays and walks would be most of a path's cost.
            if (n_dot_l <= 0.0 || dot(geometric, to_light) <= 0.0 || !any(arrived > 0.0)) {
                continue;
            }
            Light shining = frame.lights[i];
            float reach = shining.kind == 0 ? FARTHEST : length(shining.position - from);
            if (unoccluded(tlas, frame.materials, from, to_light, reach)) {
                float3 l = float3(dot(to_light, t), dot(to_light, b), n_dot_l);
                float3 later = scattered_later(v, l, lit.alpha2, lit.f0, random);
                light += throughput * (brdf(lit, to_light, n_dot_l) * n_dot_l + later) * arrived;
            }
        }
        // The next way: one lobe, picked by its estimated share, its light over the chance. The
        // diffuse's share takes the reflectance head-on, which bounds its weight. The reflectance
        // toward the view is nearly 1 where a normal map turns the normal from it: the diffuse
        // would be taken once in thousands of paths and weighed thousands of times.
        float specular_share = luminance(schlick(lit.f0, lit.n_dot_v));
        float diffuse_share = luminance(lit.diffuse_color * (1.0 - lit.diffuse_f0));
        if (specular_share + diffuse_share <= 0.0) {
            break;
        }
        float specular_chance = specular_share / (specular_share + diffuse_share);
        float3 l;
        if (next(random) < specular_chance) {
            float3 carried;
            if (!walked(v, lit.alpha2, lit.f0, random, l, carried)) {
                break;
            }
            throughput *= carried / specular_chance;
            least_roughness = min(roughness, MOST_ROUGHNESS_FLOOR);
        } else {
            float2 u = float2(next(random), next(random));
            float r = sqrt(u.x);
            float phi = 2.0 * PI * u.y;
            l = float3(r * cos(phi), r * sin(phi), sqrt(1.0 - u.x));
            // `brdf`'s diffuse over the cosine's density, l.z over π.
            float3 halfway = normalize(v + l);
            throughput *= (1.0 - schlick(lit.diffuse_f0, dot(v, halfway))) * lit.diffuse_color
                / (1.0 - specular_chance);
            shown = false;
            least_roughness = MOST_ROUGHNESS_FLOOR;
        }
        float3 out_way = t * l.x + b * l.y + lit.normal * l.z;
        if (l.z <= 0.0 || dot(out_way, geometric) <= 0.0) {
            break;
        }
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
