// Aerial perspective, included by shading.slang: the atmosphere's haze between the viewer and what
// it lights, from the froxels atmosphere.slang's `aerial_main` writes, sampled as the reference's
// RenderRayMarchingPS samples its volume (github.com/sebh/UnrealEngineSkyAtmosphere, MIT licensed,
// Copyright (c) 2020 Epic Games, Inc.).

// The froxels' slices and each one's reach, in meters: `SLICES` and `SLICE_KM` in atmosphere.slang.
static const float AERIAL_SLICES = 32.0;
static const float AERIAL_SLICE = 4000.0;

// `color`, lit at `world`, seen through the haze: dimmed by the transmittance, plus the light
// scattered toward the viewer times the sun's illuminance and the exposure. Blends the two slices
// round its distance, fading to none within the first half slice. Unchanged without an atmosphere.
float3 hazed(Frame* frame, float3 world, float3 color) {
    Aerial aerial = frame.aerial;
    if (aerial.luminance <= 0.0) {
        return color;
    }
    float4 clip = mul(aerial.clip_from_world, float4(world, 1.0));
    float2 uv = saturate(clip.xy / clip.w * 0.5 + 0.5);
    float slice = length(world - frame.camera) / AERIAL_SLICE;
    float weight = 1.0;
    if (slice < 0.5) {
        weight = saturate(slice * 2.0);
        slice = 0.5;
    }
    // Between the slices' middles, which go as the square root of the distance through their reach.
    float between = sqrt(slice / AERIAL_SLICES) * AERIAL_SLICES - 0.5;
    between = clamp(between, 0.0, AERIAL_SLICES - 1.0);
    float below = floor(between);
    float above = min(below + 1.0, AERIAL_SLICES - 1.0);
    // Half a froxel in from a tile's sides, so a sample reads its own slice alone.
    float across = clamp(uv.x * AERIAL_SLICES, 0.5, AERIAL_SLICES - 0.5);
    Texture2D froxels = textures[aerial.texture];
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    float tiles = AERIAL_SLICES * AERIAL_SLICES;
    float2 at_near = float2((across + below * AERIAL_SLICES) / tiles, uv.y);
    float2 at_far = float2((across + above * AERIAL_SLICES) / tiles, uv.y);
    float4 near = froxels.SampleLevel(clamp_, at_near, 0.0);
    float4 far = froxels.SampleLevel(clamp_, at_far, 0.0);
    float4 haze = lerp(near, far, between - below) * weight;
    return color * (1.0 - haze.a) + haze.rgb * (aerial.luminance * frame.exposure);
}
