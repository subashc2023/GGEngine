// Height fog, which a shader includes after mesh.slang, as the frame's data says it: what the fog
// does to the light from a point on its way to the eye, which fogging.slang's pass applies to
// what's opaque in the view, the sky among it, and what blends applies to itself as it draws: a
// fragment at a time, or, for what's drawn on the engine's quad, small as a sprite, a glyph or a
// particle is, at its corners, between which the fog changes too little to tell.
//
// The fog's density at height y is `density * exp(-falloff * (y - height))`. Along a ray from
// `origin` the way `way`, a unit vector, from its start, `start` along it, to a point `length`
// further, its optical depth, the integral of that density, is in closed form `density * length *
// exp(-falloff * (y_low - height)) * (1 - exp(-z)) / z`, of `z = falloff * |way.y| * length`, where
// y_low is the height of the lower of the ray's ends, where the fog's densest: from there, the fog
// thins toward the other end, whichever way the ray goes, so that nothing grows past the density
// at the ray's ends, as a closed form from the eye would where the ray falls far below it. Toward
// the sky, infinitely far, a ray that rises takes `density * exp(-falloff * (y_start - height)) /
// (falloff * way.y)`, and one that doesn't rise never comes out of the fog, and takes all of the
// light. Of what the fog takes out, it scatters back toward the eye its color of the light that
// reaches it, as the light that reaches every point of the ray is the same, which, with the light
// it lets through, `exp(-depth)`, sums to `light * (1 - exp(-depth))` (single scattering without
// shadows): the light from all around, the sky's diffuse light the way the eye looks, sampled from
// its cube as it lights Lambert's diffuse, which scatters as the sky's light from round that way
// would, or in a frame without a sky its color alone; and the sun's, by Henyey-Greenstein's phase
// function, a lobe round the way toward the sun as sharp as `lobe`, normalized over the sphere, so
// that the fog scatters no more of it than reaches it (Henyey and Greenstein, "Diffuse Radiation in
// the Galaxy", 1941). How much it lets through is kept to `least` and more, its opacity's cap.

// `Sampler::LinearClamp`'s index in the bindless set, which the sky's diffuse light is sampled by.
static const uint FOG_SAMPLER = 0;
// 1 over the 4π steradians of a sphere: what a phase function that scatters alike every way gives
// each.
static const float PER_STERADIAN = 0.0795774715;
// The most the fog's density at a ray's lower end is taken to grow past its density at its height,
// e^60, so that the optical depth stays a finite float: past it, the fog lets no light through at
// any density a world gives it anyway.
static const float DENSEST = 60.0;
// The most optical depth taken, which lets through e^-80 of the light, less than a half float holds.
static const float DEEPEST = 80.0;
// The largest a half float holds, short of infinity, which the view's colors are, and what a color
// fogged is kept to: a sharp lobe toward a bright sun scatters more.
static const float HALF_MOST = 65504.0;

// How much of the light from `along` further than the fog's start, `way` from `origin`, or from
// infinitely far if `endless`, the fog lets through, from 0 to 1.
float transmittance(Fog fog, float3 origin, float3 way, float along, bool endless) {
    if (!endless && !(along > 0.0)) {
        return 1.0;
    }
    float y = origin.y + way.y * fog.start;
    float k = fog.falloff;
    float rising = k * way.y;
    float depth;
    if (endless) {
        // Thinning ever after where it rises, without end where it doesn't.
        if (!(rising > 1e-6)) {
            return 0.0;
        }
        depth = fog.density * exp(min(k * (fog.height - y), DENSEST)) / rising;
    } else {
        float low = min(y, y + way.y * along);
        float z = abs(rising) * along;
        // The series where z is small, as the division would cancel.
        float spread =
            z < 0.01 ? along * (1.0 - z * (0.5 - z / 6.0)) : (1.0 - exp(-z)) / abs(rising);
        depth = fog.density * exp(min(k * (fog.height - low), DENSEST)) * spread;
    }
    return exp(-min(depth, DEEPEST));
}

// What the frame's fog does to the light from `distance` along `way`, a unit vector, from
// `origin`, or from infinitely far if `endless`, on its way to the eye: in rgb, the light it
// scatters toward the eye, exposed; in a, the share of the light from there it lets through.
float4 fog_along(Frame frame, float3 origin, float3 way, float distance, bool endless) {
    Fog fog = frame.fog;
    if (!(fog.density > 0.0)) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    float through =
        max(transmittance(fog, origin, way, distance - fog.start, endless), fog.least);
    float3 light = fog.ambient;
    if (frame.specular_levels > 0) {
        SamplerState clamp_ = samplers[FOG_SAMPLER];
        light *= cubes[frame.irradiance].SampleLevel(clamp_, way, 0.0).rgb;
    }
    float g = fog.lobe;
    float spread = 1.0 + g * g - 2.0 * g * dot(way, fog.to_sun);
    light += fog.sun * (PER_STERADIAN * (1.0 - g * g) / (spread * sqrt(spread)));
    return float4(light * (1.0 - through), through);
}

// The way the view sees a point along, from where, and how far along it the point is.
struct ViewRay {
    float3 origin;
    float3 way;
    float distance;
};

// The way the frame's view sees `point` along: from the camera, with perspective; or without it,
// the one way the view looks, from the camera's plane.
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

// What the frame's fog does to the light from `world` on its way to the eye, as `fog_along` gives
// it, along the way the view sees it.
float4 fog_at(Frame frame, float3 world) {
    ViewRay ray = view_ray(frame, world);
    return fog_along(frame, ray.origin, ray.way, ray.distance, false);
}

// `color`, of alpha not multiplied in, fogged by `fog`, as `fog_at` gives it where the color is:
// blended by its alpha, what it covers of the fog behind it, which the fog's pass fogged already,
// gives way to the fog in front of it.
float4 fogged(float4 fog, float4 color) {
    return float4(min(color.rgb * fog.a + fog.rgb, HALF_MOST), color.a);
}

// `color`, of alpha multiplied in, as `fogged`: so that of a color that adds its light, of alpha 0,
// the fog lets its share through and adds none of its own.
float4 fogged_premultiplied(float4 fog, float4 color) {
    return float4(min(color.rgb * fog.a + fog.rgb * color.a, HALF_MOST), color.a);
}
