// The aerial perspective, which shading.slang includes: the haze the atmosphere puts between the
// viewer and what it lights, from the froxels atmosphere.slang's `aerial_main` writes, as the
// reference's RenderRayMarchingPS samples its volume (github.com/sebh/UnrealEngineSkyAtmosphere,
// MIT licensed, Copyright (c) 2020 Epic Games, Inc.).

// The froxels' slices, and how far each reaches: `SLICES` and `SLICE_KM` in atmosphere.slang, in
// the world's units, meters.
static const float AERIAL_SLICES = 32.0;
static const float AERIAL_SLICE = 4000.0;

// `color`, a lit point's at `world`, as the view sees it through the atmosphere's haze: dimmed by
// the transmittance from it and with the light scattered toward the viewer on the way added,
// times the sun's illuminance and the exposure, from the froxels round it, between the two slices
// round its distance and fading to none at the viewer within the first half slice; or as it is in
// a frame without an atmosphere.
float3 hazed(Frame frame, float3 world, float3 color) {
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
    // Between the slices' middles, which are as far through them as the square root of how far
    // through their reach.
    float between = sqrt(slice / AERIAL_SLICES) * AERIAL_SLICES - 0.5;
    between = clamp(between, 0.0, AERIAL_SLICES - 1.0);
    float below = floor(between);
    float above = min(below + 1.0, AERIAL_SLICES - 1.0);
    // Kept half a froxel in from a tile's sides, so that a sample reads its slice's alone.
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
