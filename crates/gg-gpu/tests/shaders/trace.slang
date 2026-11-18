// Traces a row of rays straight down -z through a TLAS it takes by address, and writes what each
// hit, so that a test can read back what the GPU makes of the BLASes and TLASes frames build.

// What a ray hit: 0 for `hit` if nothing.
struct Hit {
    uint hit;
    // The instance's custom index, and the triangle's index in its BLAS.
    uint instance;
    uint primitive;
    float t;
    // The weights of the triangle's second and third corners.
    float u;
    float v;
};

struct Push {
    uint64_t tlas;
    Hit* hits;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("compute")]
[numthreads(8, 1, 1)]
void trace_main(uint3 id: SV_DispatchThreadID) {
    // From z = 0 but the sixth, from beyond the quads.
    const float3 origins[8] = {
        float3(0.5, -0.5, 0.0),
        float3(-0.25, 0.5, 0.0),
        float3(1.25, 0.0, 0.0),
        float3(10.0, 0.0, 0.0),
        float3(0.0, 3.0, 0.0),
        float3(0.5, -0.5, -10.0),
        float3(-0.9, -0.2, 0.0),
        float3(0.2, 0.9, 0.0),
    };
    RayDesc ray;
    ray.Origin = origins[id.x];
    ray.Direction = float3(0.0, 0.0, -1.0);
    ray.TMin = 0.0;
    ray.TMax = 100.0;
    RayQuery<RAY_FLAG_NONE> query;
    query.TraceRayInline(RaytracingAccelerationStructure(push.tlas), RAY_FLAG_NONE, 0xff, ray);
    while (query.Proceed()) {
    }
    Hit hit = {};
    if (query.CommittedStatus() == COMMITTED_TRIANGLE_HIT) {
        float2 weights = query.CommittedTriangleBarycentrics();
        hit.hit = 1;
        hit.instance = query.CommittedInstanceID();
        hit.primitive = query.CommittedPrimitiveIndex();
        hit.t = query.CommittedRayT();
        hit.u = weights.x;
        hit.v = weights.y;
    }
    push.hits[id.x] = hit;
}
