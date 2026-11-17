// Height fog, included after mesh.slang: what the frame's fog does to light from a point on its
// way to the eye. fogging.slang's pass applies it to what's opaque, the sky included. What blends
// applies it to itself: per fragment, or, on the engine's quad (sprites, glyphs, particles), at
// its corners, between which the fog changes too little to tell.
//
// Density at height y is `density * exp(-falloff * (y - height))`. Along a ray from `origin` the
// unit way `way`, from `start` along it to a point `length` further, the optical depth is
// `density * length * exp(-falloff * (y_low - height)) * (1 - exp(-z)) / z`, where
// `z = falloff * |way.y| * length` and y_low is the lower end's height, where the fog's densest.
// Taken from there, nothing grows past the ends' density, as a form from the eye would where the
// ray falls far below it. To the sky, a rising ray takes `density * exp(-falloff * (y_start -
// height)) / (falloff * way.y)`; any other takes all the light.
//
// Each point of the ray gets the same light, so in-scattering plus what's let through,
// `exp(-depth)`, is `light * (1 - exp(-depth))` (single scattering, no shadows). The light:
// - ambient: the sky's diffuse light the way the eye looks, from its irradiance cube, as the sky's
//   light from round that way would scatter; without a sky, the fog's color alone.
// - the sun's: Henyey-Greenstein's phase function, a lobe toward the sun as sharp as `lobe`,
//   normalized over the sphere so it scatters no more than reaches it (Henyey and Greenstein,
//   "Diffuse Radiation in the Galaxy", 1941).
//
// What's let through is `least` or more, the opacity's cap.

#include "common.slang"

// 1 over a sphere's 4π steradians: an isotropic phase function's value.
static const float PER_STERADIAN = 0.0795774715;
// The most the density's exponent below the fog's height grows, e^60, so the optical depth stays
// finite. Past it, no light gets through at any density anyway.
static const float DENSEST = 60.0;
// The most optical depth taken: e^-80 lets through less than a half float holds.
static const float DEEPEST = 80.0;

// The share of the light, 0 to 1, the fog lets through from `along` past its start, `way` from
// `origin`, or from infinitely far if `endless`.
float transmittance(Fog fog, float3 origin, float3 way, float along, bool endless) {
    if (!endless && !(along > 0.0)) {
        return 1.0;
    }
    float y = origin.y + way.y * fog.start;
    float k = fog.falloff;
    float rising = k * way.y;
    float depth;
    if (endless) {
        // Rising, it thins forever; otherwise it never ends.
        if (!(rising > 1e-6)) {
            return 0.0;
        }
        depth = fog.density * exp(min(k * (fog.height - y), DENSEST)) / rising;
    } else {
        float low = min(y, y + way.y * along);
        float z = abs(rising) * along;
        // The series for small z, where the division would cancel.
        float spread =
            z < 0.01 ? along * (1.0 - z * (0.5 - z / 6.0)) : (1.0 - exp(-z)) / abs(rising);
        depth = fog.density * exp(min(k * (fog.height - low), DENSEST)) * spread;
    }
    return exp(-min(depth, DEEPEST));
}

// What the frame's fog does to light from `distance` along `way`, a unit vector, from `origin`, or
// from infinitely far if `endless`: rgb, the light it scatters toward the eye, exposed; a, the
// share it lets through.
float4 fog_along(Frame frame, float3 origin, float3 way, float distance, bool endless) {
    Fog fog = frame.fog;
    if (!(fog.density > 0.0)) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    float through =
        max(transmittance(fog, origin, way, distance - fog.start, endless), fog.least);
    float3 light = fog.ambient;
    if (frame.specular_levels > 0) {
        SamplerState clamp_ = samplers[LINEAR_CLAMP];
        light *= cubes[frame.irradiance].SampleLevel(clamp_, way, 0.0).rgb;
    }
    float g = fog.lobe;
    float spread = 1.0 + g * g - 2.0 * g * dot(way, fog.to_sun);
    light += fog.sun * (PER_STERADIAN * (1.0 - g * g) / (spread * sqrt(spread)));
    return float4(light * (1.0 - through), through);
}

// The ray the view sees a point along, and the point's distance on it.
struct ViewRay {
    float3 origin;
    float3 way;
    float distance;
};

// The view's ray to `point`: from the camera, with perspective; without, the view's one way, from
// the camera's plane.
ViewRay view_ray(Frame frame, float3 point) {
    ViewRay ray;
    if (frame.eye.w != 0.0) {
        ray.origin = frame.eye.xyz;
        float3 to = point - ray.origin;
        ray.distance = length(to);
        ray.way = ray.distance > 0.0 ? to / ray.distance : float3(0.0, 1.0, 0.0);
    } else {
        ray.way = -frame.eye.xyz;
        ray.distance = dot(point - frame.camera, ray.way);
        ray.origin = point - ray.way * ray.distance;
    }
    return ray;
}

// `fog_along` for `world`, along the view's ray to it.
float4 fog_at(Frame frame, float3 world) {
    ViewRay ray = view_ray(frame, world);
    return fog_along(frame, ray.origin, ray.way, ray.distance, false);
}

// `color`, alpha not multiplied in, fogged by `fog` from `fog_at` where it is. Blended by its
// alpha, it trades the fog behind it, which the fog's pass applied, for the fog in front. Kept to
// `BRIGHTEST`: a sharp lobe toward a bright sun scatters more.
float4 fogged(float4 fog, float4 color) {
    return float4(min(color.rgb * fog.a + fog.rgb, BRIGHTEST), color.a);
}

// `fogged` for alpha multiplied in: of an additive color, alpha 0, the fog lets its share through
// and adds none of its own.
float4 fogged_premultiplied(float4 fog, float4 color) {
    return float4(min(color.rgb * fog.a + fog.rgb * color.a, BRIGHTEST), color.a);
}
