// A sky made from its sun: Hillaire's atmosphere ("A Scalable and Production Ready Sky and
// Atmosphere Rendering Technique", EGSR 2020), as his reference implementation has it
// (github.com/sebh/UnrealEngineSkyAtmosphere at its RenderSkyCommon.hlsl and
// RenderSkyRayMarching.hlsl, MIT licensed, Copyright (c) 2020 Epic Games, Inc.), Rayleigh's and
// Mie's scattering and ozone's absorption over a planet of Earth's radius, in kilometers, +Y up and
// the planet's center straight below the viewer. Compute passes make its tables each frame, every
// value of them for a sun of an illuminance of 1, its color (`tint`) in: the transmittance from a
// height out along a way, by Bruneton's map of the height and the cosine of the way's zenith
// angle; the light scattered many times over, Ψms, by the sun's zenith cosine and the height, from
// 64 ways round each texel; the sky as the viewer sees it, by the zenith angle, its texels crowded
// toward the horizon, and the way round from the sun's; the aerial perspective, 32 slices of 32 by
// 32 froxels of the view, each the light scattered toward the viewer from as far as its slice and
// the transmittance there, the slices side by side in one image; and from the sky's table, the
// sky's cube, which sky.rs prefilters as an image sky's. The view draws the sky from its table,
// and the sun's disk through the transmittance. `Push` and `Atmosphere` in src/atmosphere.rs,
// which `Push` and `Atmosphere` here match field for field.

#include "equirect.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// The atmosphere, in kilometers, and how the frame lights it and sees it.
struct Atmosphere {
    // From clip space to the ways the view sees, unjittered, as sky.slang's `world_from_clip`:
    // what the aerial perspective's froxels see along.
    float4x4 ways_from_clip;
    // Rayleigh's scattering at the ground, per kilometer, and Mie's, alike for every channel.
    float3 rayleigh;
    float mie_scattering;
    // Ozone's absorption where it's densest, per kilometer, and Mie's extinction, its scattering
    // and absorption together.
    float3 ozone;
    float mie_extinction;
    // The ground's albedo, and the asymmetry of Mie's phase function, Henyey-Greenstein's g.
    float3 ground_albedo;
    float mie_anisotropy;
    // The unit way toward the sun, and how far the viewer is above the ground: a height rather
    // than a radius, as an f32 keeps meters in it, which it loses at the planet's radius.
    float3 sun;
    float height;
    // The sun's color, as a share of its illuminance, and the cosine of its disk's radius.
    float3 tint;
    float sun_cosine;
};

struct Push {
    // The sky's draw's: from clip space to the ways the view sees, drawn off by the frame's offset.
    float4x4 world_from_clip;
    Atmosphere* atmosphere;
    // The tables, textures of the bindless set's: the transmittance, the light scattered many times
    // over and the sky's.
    uint transmittance;
    uint multiple;
    uint sky_view;
    // The storage slot written.
    uint target;
    // The side of the cube's face written.
    uint size;
    // The sky's draw's: what its light is multiplied by, the sun's illuminance times the exposure;
    // and the sun's disk's luminance for an illuminance of 1, 1 over the solid angle it spans.
    float scale;
    float disk;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// The planet's radius and the atmosphere's top's, the scale heights its Rayleigh and Mie
// scattering thin by, and where ozone is densest and how far above and below it thins to none.
static const float GROUND = 6360.0;
static const float TOP = 6460.0;
static const float RAYLEIGH_HEIGHT = 8.0;
static const float MIE_HEIGHT = 1.2;
static const float OZONE_HEIGHT = 25.0;
static const float OZONE_WIDTH = 15.0;
// How far above the ground a point on it looks for the sun from, and the least the multiple
// scattering's table's heights start above it: the reference's PLANET_RADIUS_OFFSET.
static const float OFFSET = 0.01;
// The tables' sizes, as src/atmosphere.rs makes them; the aerial perspective's slices, and how
// far each reaches, squared, as `slice_reach` says.
static const float2 TRANSMITTANCE_SIZE = float2(256.0, 64.0);
static const float MULTIPLE_SIZE = 32.0;
static const float2 SKY_VIEW_SIZE = float2(192.0, 108.0);
static const uint SLICES = 32;
static const float SLICE_KM = 4.0;
// How many steps the transmittance and the light scattered many times over take, and the least and
// most the sky's table takes, by how far it reaches.
static const uint TRANSMITTANCE_STEPS = 40;
static const float MULTIPLE_STEPS = 20.0;
static const float2 SKY_STEPS = float2(4.0, 14.0);
// The largest a half float holds, short of infinity, which the view's colors are.
static const float BRIGHTEST = 65504.0;

// What scatters and what takes light away at a point `radius` from the planet's center.
struct Medium {
    float3 rayleigh;
    float3 mie;
    float3 scattering;
    float3 extinction;
};

Medium medium_at(Atmosphere atmosphere, float height) {
    float mie = exp(-height / MIE_HEIGHT);
    float ozone = saturate(1.0 - abs(height - OZONE_HEIGHT) / OZONE_WIDTH);
    Medium medium;
    medium.rayleigh = atmosphere.rayleigh * exp(-height / RAYLEIGH_HEIGHT);
    medium.mie = float3(atmosphere.mie_scattering * mie);
    medium.scattering = medium.rayleigh + medium.mie;
    medium.extinction =
        medium.rayleigh + atmosphere.mie_extinction * mie + atmosphere.ozone * ozone;
    return medium;
}

// Points are heights above the ground, not radii: at the planet's radius f32s are half a meter
// apart, and the squares of radii 4 km² apart, a third of a meter of height, so that what's found
// from radii a few meters above the ground is found by rounding, which no two GPUs do alike. A
// point's radius is used only where meters don't count.

// How much higher than a point `radius` from the planet's center a point `t` along a way of zenith
// cosine `mu` from it is: the difference of their radii's squares over their sum, which keeps the
// meters their difference loses.
float rise(float radius, float mu, float t) {
    float squares = t * (t + 2.0 * radius * mu);
    return squares / (sqrt(max(radius * radius + squares, 0.0)) + radius);
}

// How far along a way of zenith cosine `mu`, from a point `height` above the ground, a ray first
// meets the sphere `sphere` above the ground round the planet's center, the far side from inside
// it or on it; -1 if it doesn't. Its square's constant term from the heights, and the root whose
// terms would cancel from the other's, by their product, so that a point a meter from the sphere
// is on its side of it.
float nearest(float height, float mu, float sphere) {
    float radius = GROUND + height;
    float b = radius * mu;
    float c = (height - sphere) * (radius + GROUND + sphere);
    float discriminant = b * b - c;
    if (discriminant < 0.0) {
        return -1.0;
    }
    float root = sqrt(discriminant);
    float near;
    float far;
    if (b >= 0.0) {
        near = -b - root;
        far = b + root > 0.0 ? -c / (b + root) : 0.0;
    } else {
        far = -b + root;
        near = c / far;
    }
    if (near < 0.0 && far < 0.0) {
        return -1.0;
    }
    if (near < 0.0) {
        return max(0.0, far);
    }
    if (far < 0.0) {
        return max(0.0, near);
    }
    return max(0.0, min(near, far));
}

// Where a texel's center `u`, of a table `size` texels across, falls from 0 to 1 when the first
// and last texels' centers stand for 0 and 1; and its inverse.
float unit_of(float u, float size) {
    return (u - 0.5 / size) * (size / (size - 1.0));
}
float texel_of(float u, float size) {
    return u * ((size - 1.0) / size) + 0.5 / size;
}

// The transmittance's table's coordinates of a point `height` above the ground and the cosine `mu`
// of a way's zenith angle there, by Bruneton's map: the height across the horizon's distance down,
// and how far the way reaches out of the atmosphere, between the least and most it could, across.
float2 transmittance_uv(float height, float mu) {
    float radius = GROUND + height;
    float horizon = sqrt(TOP * TOP - GROUND * GROUND);
    float rho = sqrt(max(0.0, height * (radius + GROUND)));
    float discriminant = radius * radius * (mu * mu - 1.0) + TOP * TOP;
    float reach = max(0.0, -radius * mu + sqrt(max(discriminant, 0.0)));
    float least = TOP - GROUND - height;
    float most = rho + horizon;
    return float2((reach - least) / (most - least), rho / horizon);
}

// The point's height and way's cosine that the transmittance's table's coordinates `uv` stand for.
void transmittance_params(float2 uv, out float height, out float mu) {
    float horizon = sqrt(TOP * TOP - GROUND * GROUND);
    float rho = horizon * uv.y;
    float radius = sqrt(rho * rho + GROUND * GROUND);
    height = rho * rho / (radius + GROUND);
    float least = TOP - GROUND - height;
    float most = rho + horizon;
    float reach = least + uv.x * (most - least);
    mu = reach == 0.0 ? 1.0
                      : (horizon * horizon - rho * rho - reach * reach) / (2.0 * radius * reach);
    mu = clamp(mu, -1.0, 1.0);
}

// The share of light that comes from outside the atmosphere to a point `height` above the ground
// along a way of zenith cosine `mu`, as the table holds it: of ways above the horizon, which the
// ground leaves alone.
float3 transmittance_to_top(float height, float mu) {
    float2 uv = transmittance_uv(height, mu);
    return textures[push.transmittance].SampleLevel(samplers[LINEAR_CLAMP], uv, 0.0).rgb;
}

// The light scattered many times over toward a point `height` above the ground, under a sun of
// zenith cosine `mu` there, for a medium that scatters 1.
float3 multiple_at(float height, float mu) {
    float2 uv = saturate(float2(mu * 0.5 + 0.5, height / (TOP - GROUND)));
    uv = float2(texel_of(uv.x, MULTIPLE_SIZE), texel_of(uv.y, MULTIPLE_SIZE));
    return textures[push.multiple].SampleLevel(samplers[LINEAR_CLAMP], uv, 0.0).rgb;
}

float rayleigh_phase(float cosine) {
    return 3.0 / (16.0 * PI) * (1.0 + cosine * cosine);
}

// Henyey-Greenstein's phase function of asymmetry `g`, at the cosine of the angle between the way
// light goes on and the way it came, as the reference has it for Mie.
float mie_phase(float g, float cosine) {
    float denominator = 1.0 + g * g - 2.0 * g * cosine;
    return (1.0 - g * g) / (4.0 * PI * denominator * sqrt(denominator));
}

// The optical depth from a point `height` above the ground out of the atmosphere along a way of
// zenith cosine `mu`, or to the ground, by the midpoints of even steps.
float3 optical_depth(Atmosphere atmosphere, float height, float mu) {
    float radius = GROUND + height;
    // To the ground, if the way goes into it, and otherwise out of the atmosphere.
    float bottom = nearest(height, mu, 0.0);
    float top = max(nearest(height, mu, TOP - GROUND), 0.0);
    float end = bottom > 0.0 ? min(top, bottom) : top;
    float step = end / float(TRANSMITTANCE_STEPS);
    float3 depth = float3(0.0);
    for (uint i = 0; i < TRANSMITTANCE_STEPS; i++) {
        float t = (float(i) + 0.5) * step;
        depth += medium_at(atmosphere, height + rise(radius, mu, t)).extinction;
    }
    return depth * step;
}

// What reaches a point along a ray through the atmosphere.
struct Scattered {
    // The light scattered toward it, for a sun of `sunlight`.
    float3 luminance;
    // The share of light from the ray's end that reaches it.
    float3 transmittance;
    // The light scattered toward it once, from light of 1 coming every way: f_ms's integrand.
    float3 multiple;
};

// The light scattered toward a point `height` above the ground along `way`, a unit vector, the
// point straight above the planet's center, from as far as the atmosphere, the ground or `most`
// reach, under a sun the unit way `to_sun` of `sunlight`, as the reference's
// IntegrateScatteredLuminance has it: in `samples` steps, or, if `variable`, from 4 to 14 by how
// far the ray reaches, crowded toward its start; by Rayleigh's and Mie's phase functions if
// `phased`, and every way alike otherwise; with the light scattered many times over if `multiply`;
// and with the light the ground reflects at its end if `ground`.
Scattered scattered(Atmosphere atmosphere, float height, float3 way, float3 to_sun,
                    float3 sunlight, bool ground, float samples, bool variable, bool phased,
                    float most, bool multiply) {
    Scattered out;
    out.luminance = float3(0.0);
    out.transmittance = float3(1.0);
    out.multiple = float3(0.0);
    float radius = GROUND + height;
    float3 from = float3(0.0, radius, 0.0);
    float bottom = nearest(height, way.y, 0.0);
    float top = nearest(height, way.y, TOP - GROUND);
    float end = 0.0;
    if (bottom < 0.0) {
        if (top < 0.0) {
            return out;
        }
        end = top;
    } else if (top > 0.0) {
        end = min(top, bottom);
    }
    end = min(end, most);
    float count = samples;
    float whole = samples;
    float whole_end = end;
    if (variable) {
        count = lerp(SKY_STEPS.x, SKY_STEPS.y, saturate(end * 0.01));
        whole = floor(count);
        // To the end of the last whole step.
        whole_end = end * whole / count;
    }
    float cosine = dot(to_sun, way);
    float mie = mie_phase(atmosphere.mie_anisotropy, cosine);
    float rayleigh = rayleigh_phase(cosine);
    float t = 0.0;
    float dt = end / count;
    for (float s = 0.0; s < count; s += 1.0) {
        if (variable) {
            float t0 = s / whole;
            float t1 = (s + 1.0) / whole;
            t0 = whole_end * t0 * t0;
            t1 = t1 * t1 > 1.0 ? end : whole_end * t1 * t1;
            t = t0 + (t1 - t0) * 0.3;
            dt = t1 - t0;
        } else {
            // From the step before's point exactly, which the light scattered many times over
            // needs to be right.
            float next = end * (s + 0.3) / count;
            dt = next - t;
            t = next;
        }
        float sun_mu = dot(to_sun, normalize(from + t * way));
        float lifted = rise(radius, way.y, t);
        float there = height + lifted;
        Medium medium = medium_at(atmosphere, there);
        float3 extinction = max(medium.extinction, float3(1e-9));
        float3 through = exp(-medium.extinction * dt);
        float3 to_sun_transmittance = transmittance_to_top(there, sun_mu);
        float3 phase_scattering = phased ? medium.mie * mie + medium.rayleigh * rayleigh
                                         : medium.scattering / (4.0 * PI);
        // In the planet's shadow, the point looked out from a little lower: `OFFSET` lower, taken
        // from the viewer's height before the rise is added, so that a viewer held at `OFFSET`
        // leaves the rise whole.
        float shadow = nearest(height - OFFSET + lifted, sun_mu, 0.0) >= 0.0 ? 0.0 : 1.0;
        float3 many = multiply ? multiple_at(there, sun_mu) : float3(0.0);
        float3 lit = shadow * to_sun_transmittance * phase_scattering;
        float3 source = sunlight * (lit + many * medium.scattering);
        // Each integrated over its step, as the extinction there takes it away (Hillaire,
        // "Physically Based and Unified Volumetric Rendering in Frostbite", 2015).
        out.multiple += out.transmittance * (medium.scattering - medium.scattering * through)
            / extinction;
        out.luminance += out.transmittance * (source - source * through) / extinction;
        out.transmittance *= through;
    }
    if (ground && end == bottom && bottom > 0.0) {
        float sun_mu = dot(to_sun, normalize(from + bottom * way));
        float3 to_sun_transmittance = transmittance_to_top(0.0, sun_mu);
        out.luminance += sunlight * to_sun_transmittance * out.transmittance * saturate(sun_mu)
            * atmosphere.ground_albedo / PI;
    }
    return out;
}

// The angle from straight down to the horizon from a point `height` above the ground.
float below_horizon(float height) {
    float radius = GROUND + height;
    return acos(sqrt(max(height * (radius + GROUND), 0.0)) / radius);
}

// The sky's table's coordinates of a way of zenith cosine `view_mu` whose way round from the sun's
// has the cosine `light_view`, from a point `height` above the ground, which meets the ground if
// `ground`: the zenith angle down, squared toward the horizon from above and below it, and the way
// round across, squared toward the sun's.
float2 sky_view_uv(bool ground, float view_mu, float light_view, float height) {
    float beta = below_horizon(height);
    float zenith_horizon = PI - beta;
    float2 uv;
    if (!ground) {
        float coord = acos(view_mu) / zenith_horizon;
        uv.y = (1.0 - sqrt(max(1.0 - coord, 0.0))) * 0.5;
    } else {
        float coord = (acos(view_mu) - zenith_horizon) / beta;
        uv.y = sqrt(max(coord, 0.0)) * 0.5 + 0.5;
    }
    uv.x = sqrt(saturate(-light_view * 0.5 + 0.5));
    return float2(texel_of(uv.x, SKY_VIEW_SIZE.x), texel_of(uv.y, SKY_VIEW_SIZE.y));
}

// The way's zenith cosine and its way round from the sun's that the sky's table's coordinates `uv`
// stand for, from a point `height` above the ground.
void sky_view_params(float2 uv, float height, out float view_mu, out float light_view) {
    uv = float2(unit_of(uv.x, SKY_VIEW_SIZE.x), unit_of(uv.y, SKY_VIEW_SIZE.y));
    float beta = below_horizon(height);
    float zenith_horizon = PI - beta;
    if (uv.y < 0.5) {
        float coord = 1.0 - 2.0 * uv.y;
        view_mu = cos(zenith_horizon * (1.0 - coord * coord));
    } else {
        float coord = uv.y * 2.0 - 1.0;
        view_mu = cos(zenith_horizon + beta * coord * coord);
    }
    light_view = 1.0 - 2.0 * uv.x * uv.x;
}

// The sky the viewer sees along `way`, a unit vector in the world, from its table, for a sun of an
// illuminance of 1.
float3 sky_view_at(Atmosphere atmosphere, float3 way) {
    float lengths = length(way.xz) * length(atmosphere.sun.xz);
    float light_view =
        lengths > 0.0 ? clamp(dot(way.xz, atmosphere.sun.xz) / lengths, -1.0, 1.0) : 1.0;
    bool ground = nearest(atmosphere.height, way.y, 0.0) >= 0.0;
    float2 uv = sky_view_uv(ground, clamp(way.y, -1.0, 1.0), light_view, atmosphere.height);
    return textures[push.sky_view].SampleLevel(samplers[LINEAR_CLAMP], uv, 0.0).rgb;
}

// The transmittance's table, a texel each.
[shader("compute")]
[numthreads(8, 8, 1)]
void transmittance_main(uint3 id: SV_DispatchThreadID) {
    if (any(id.xy >= uint2(TRANSMITTANCE_SIZE))) {
        return;
    }
    Atmosphere atmosphere = *push.atmosphere;
    float height;
    float mu;
    transmittance_params((float2(id.xy) + 0.5) / TRANSMITTANCE_SIZE, height, mu);
    float3 transmittance = exp(-optical_depth(atmosphere, height, mu));
    storage[push.target][uint3(id.xy, 0)] = float4(transmittance, 1.0);
}

// What the threads of a texel of the multiple scattering's table each find along their way, before
// they're summed.
groupshared float3 shared_luminance[64];
groupshared float3 shared_multiple[64];

// The light scattered many times over's table, a workgroup a texel and a thread for each of 64
// ways round it, evenly over the sphere: the light scattered twice toward the point, L_2nd, over
// 1 less f_ms, the share of light a point gets back from one scattering, so that the orders after
// sum as a geometric series (the paper's equations 5 to 10).
[shader("compute")]
[numthreads(1, 1, 64)]
void multiple_main(uint3 id: SV_DispatchThreadID, uint3 thread: SV_GroupThreadID) {
    Atmosphere atmosphere = *push.atmosphere;
    float2 uv = (float2(id.xy) + 0.5) / MULTIPLE_SIZE;
    uv = float2(unit_of(uv.x, MULTIPLE_SIZE), unit_of(uv.y, MULTIPLE_SIZE));
    float sun_mu = uv.x * 2.0 - 1.0;
    float3 to_sun = float3(sqrt(saturate(1.0 - sun_mu * sun_mu)), sun_mu, 0.0);
    float height = saturate(uv.y + OFFSET) * (TOP - GROUND - OFFSET);
    uint i = thread.z;
    float theta = 2.0 * PI * (float(i / 8) + 0.5) / 8.0;
    float phi = acos(1.0 - 2.0 * (float(i % 8) + 0.5) / 8.0);
    float3 way = float3(cos(theta) * sin(phi), cos(phi), sin(theta) * sin(phi));
    Scattered found = scattered(atmosphere, height, way, to_sun, float3(1.0), true,
                                MULTIPLE_STEPS, false, false, 9.0e6, false);
    // Each way's share of the sphere's solid angle.
    shared_luminance[i] = found.luminance * (4.0 * PI / 64.0);
    shared_multiple[i] = found.multiple * (4.0 * PI / 64.0);
    GroupMemoryBarrierWithGroupSync();
    for (uint width = 32; width > 0; width /= 2) {
        if (i < width) {
            shared_luminance[i] += shared_luminance[i + width];
            shared_multiple[i] += shared_multiple[i + width];
        }
        GroupMemoryBarrierWithGroupSync();
    }
    if (i > 0) {
        return;
    }
    // By the isotropic phase function.
    float3 multiple = shared_multiple[0] / (4.0 * PI);
    float3 second = shared_luminance[0] / (4.0 * PI);
    float3 sum = second / max(1.0 - multiple, float3(1e-3));
    storage[push.target][uint3(id.xy, 0)] = float4(sum, 1.0);
}

// The sky's table, a texel each, as the viewer sees it, the ground's light where its ways meet it.
[shader("compute")]
[numthreads(8, 8, 1)]
void sky_view_main(uint3 id: SV_DispatchThreadID) {
    if (any(id.xy >= uint2(SKY_VIEW_SIZE))) {
        return;
    }
    Atmosphere atmosphere = *push.atmosphere;
    float view_mu;
    float light_view;
    sky_view_params((float2(id.xy) + 0.5) / SKY_VIEW_SIZE, atmosphere.height, view_mu,
                    light_view);
    float sun_mu = clamp(atmosphere.sun.y, -1.0, 1.0);
    float3 to_sun = float3(sqrt(saturate(1.0 - sun_mu * sun_mu)), sun_mu, 0.0);
    float view_sin = sqrt(saturate(1.0 - view_mu * view_mu));
    float3 way = float3(view_sin * light_view, view_mu,
                        view_sin * sqrt(saturate(1.0 - light_view * light_view)));
    Scattered found = scattered(atmosphere, atmosphere.height, way, to_sun, atmosphere.tint,
                                true, 30.0, true, true, 9.0e6, true);
    storage[push.target][uint3(id.xy, 0)] = float4(found.luminance, 1.0);
}

// How far the middle of the aerial perspective's slice `slice` reaches, in kilometers: crowded
// toward the viewer, as the square of how far through the slices it is.
float slice_reach(uint slice) {
    float through = (float(slice) + 0.5) / float(SLICES);
    return through * through * float(SLICES) * SLICE_KM;
}

// The aerial perspective's froxels, a thread each: the light scattered toward the viewer along its
// froxel's way from as far as its slice reaches, and 1 less the transmittance from there, the mean
// of its channels'. Its slice's tile, `SLICES` froxels across, is the slice's place across.
[shader("compute")]
[numthreads(8, 8, 1)]
void aerial_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= SLICES || id.y >= SLICES) {
        return;
    }
    Atmosphere atmosphere = *push.atmosphere;
    float2 clip = (float2(id.xy) + 0.5) / float(SLICES) * 2.0 - 1.0;
    float4 point = mul(atmosphere.ways_from_clip, float4(clip, 1.0, 1.0));
    float3 way = normalize(point.xyz / point.w);
    float height = atmosphere.height;
    float radius = GROUND + height;
    float reach = slice_reach(id.z);
    // A froxel under the ground, or less than `OFFSET` above it, is moved up onto it, its way and
    // reach to it found again: from the heights, and the angle at the planet's center, whose sine
    // and its 1 less cosine keep the meters the difference of two radii loses.
    float raised = OFFSET + 0.001;
    if (height + rise(radius, way.y, reach) <= OFFSET) {
        float3 to = float3(0.0, radius, 0.0) + reach * way;
        float far = length(to);
        float sine = length(to.xz) / far;
        float bend = sine * sine / (1.0 + to.y / far);
        float3 moved = float3(to.x, 0.0, to.z) / far * (GROUND + raised);
        moved.y = (raised - height) - (GROUND + raised) * bend;
        reach = length(moved);
        way = moved / reach;
    }
    float samples = max(1.0, float(id.z + 1) * 2.0);
    Scattered found = scattered(atmosphere, height, way, atmosphere.sun, atmosphere.tint, false,
                                samples, false, true, reach, true);
    float transmittance = dot(found.transmittance, float3(1.0 / 3.0));
    storage[push.target][uint3(id.x + id.z * SLICES, id.y, 0)] =
        float4(found.luminance, 1.0 - transmittance);
}

// The sky's cube's first level, a texel each, from the sky's table, without the sun's disk.
[shader("compute")]
[numthreads(8, 8, 1)]
void cube_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    Atmosphere atmosphere = *push.atmosphere;
    float3 way = cube_direction(id.z, float2(id.xy) + 0.5, float(push.size));
    storage[push.target][id] = float4(sky_view_at(atmosphere, way), 1.0);
}

struct Varyings {
    float4 position : SV_Position;
    float2 clip;
};

// A triangle over the whole viewport, from three vertices and no buffer, at depth 0, as sky.slang
// draws one.
[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID) {
    float2 corner = float2((index << 1) & 2, index & 2) * 2.0 - 1.0;
    Varyings out;
    out.position = float4(corner, 0.0, 1.0);
    out.clip = corner;
    return out;
}

// The color, no ID, and zeros at the motion's location, which the pipeline doesn't write, but
// lavapipe may, as `GraphicsDesc::format` in gg-gpu says.
struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
    float4 motion : SV_Target2;
};

// The sky behind the world, each pixel what the sky's table holds the way it sees, and the sun's
// disk there, unless the ground is in the way, through the transmittance, times the sun's
// illuminance and the exposure, writing no ID.
[shader("fragment")]
Targets fragment_main(Varyings input) {
    // The near plane, where reversed depth is 1, which perspective's inverse takes to a point.
    float4 point = mul(push.world_from_clip, float4(input.clip, 1.0, 1.0));
    float3 way = normalize(point.xyz / point.w);
    Atmosphere atmosphere = *push.atmosphere;
    float3 color = sky_view_at(atmosphere, way);
    if (dot(way, atmosphere.sun) > atmosphere.sun_cosine
        && nearest(atmosphere.height, way.y, 0.0) < 0.0) {
        float3 through = transmittance_to_top(atmosphere.height, way.y);
        color += through * atmosphere.tint * push.disk;
    }
    Targets out;
    out.color = float4(min(color * push.scale, BRIGHTEST), 1.0);
    out.id = uint2(0, 0);
    out.motion = float4(0.0);
    return out;
}
