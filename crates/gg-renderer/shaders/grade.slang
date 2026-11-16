// The color grade, as src/grade.rs's `Grade` says and its `graded` computes it on the CPU: the
// view's light white balanced, its contrast raised or lowered round mid gray, through ASC CDL's
// slope, offset and power, and saturated, in that order, each step's light within a half float's
// largest, so that no grade makes a color infinite or no number.
//
// `Grade` matches `GradeData` in src/grade.rs field for field.

#include "common.slang"

struct Grade {
    // The white balance's rows, in linear sRGB, and in their fourth components the contrast and
    // the saturation.
    float4 balance[3];
    // ASC CDL's slope, offset and power, each channel's.
    float4 slope;
    float4 offset;
    float4 power;
};

static const float MID_GRAY = 0.18;

// What the view's colors are weighed by for their luminance: Rec. 709's, the sRGB primaries' own,
// as ASC CDL's saturation weighs them.
static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);

// `light` raised to `power`, each channel, but 0 to any, 0 among them, rather than no number.
float3 raised(float3 light, float3 power) {
    return exp2(power * log2(max(light, 1.0e-10)));
}

// `light`, which isn't negative, graded by `grade`.
float3 graded(float3 light, Grade grade) {
    float contrast = grade.balance[0].w;
    float saturation = grade.balance[1].w;
    light = float3(dot(grade.balance[0].xyz, light), dot(grade.balance[1].xyz, light),
                   dot(grade.balance[2].xyz, light));
    light = clamp(light, 0.0, BRIGHTEST);
    light = min(MID_GRAY * raised(light / MID_GRAY, float3(contrast)), BRIGHTEST);
    light = max(light * grade.slope.rgb + grade.offset.rgb, 0.0);
    light = min(raised(light, grade.power.rgb), BRIGHTEST);
    float luma = dot(light, LUMA);
    return clamp(luma + saturation * (light - luma), 0.0, BRIGHTEST);
}
