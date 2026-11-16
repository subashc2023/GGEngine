// Lines between two points in a draw's space, on the engine's quad, which the vertex stage lays
// along the line as the view shows it, `width` view pixels wide at any distance. Ends are squared
// off half a width past the points, so a rectangle's sides meet. Edges are smoothed over a pixel;
// thinner than a pixel draws a pixel wide, as much fainter. Cut at the near plane, as what's
// behind the camera would show mirrored. An instance per line, with its own transform, ID and
// parameters, drawn by its alpha. `Line` matches `Params` in src/line.rs byte for byte.

#include "mesh.slang"

struct Line {
    float3 from;
    float3 to;
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // In the view's pixels.
    float width;
};

[[vk::push_constant]] ConstantBuffer<Push<Corner, Line>> push;

struct Varyings {
    float4 position : SV_Position;
    // From the line's middle as the view shows it, in pixels: along it, first point toward second,
    // then across.
    noperspective float2 at;
    // From its middle, along and across, in pixels, to where it covers half a pixel.
    nointerpolation float2 reach;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

// How far past the reach the quad goes, in pixels, to fit the smoothed edges.
static const float MARGIN = 1.0;

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Corner corner = push.vertices[index];
    Line line = push.params[instance];
    float4x4 placed = mul(push.clip_from_world, push.transforms[instance]);
    float4 from = mul(placed, float4(line.from, 1.0));
    float4 to = mul(placed, float4(line.to, 1.0));
    Varyings out;
    out.instance = instance;
    out.id = push.ids[instance];
    // Before the near plane, where depth runs down from 1, z is at most w. A line wholly behind it
    // draws nothing: every corner is outside the view.
    float before = from.w - from.z;
    float after = to.w - to.z;
    if (before < 0.0 && after < 0.0) {
        out.position = float4(2.0, 2.0, 2.0, 1.0);
        out.at = float2(0.0, 0.0);
        out.reach = float2(0.0, 0.0);
        return out;
    }
    if (before < 0.0) {
        from = lerp(from, to, before / (before - after));
    } else if (after < 0.0) {
        to = lerp(to, from, after / (after - before));
    }
    // Where its ends show, in pixels from the view's middle.
    float2 half_view = push.view * 0.5;
    float2 start = from.xy / from.w * half_view;
    float2 along = to.xy / to.w * half_view - start;
    float span = length(along);
    // Seen end on it has no direction: any gives a square as wide as it.
    float2 forward = span > 1e-6 ? along / span : float2(1.0, 0.0);
    float2 sideways = float2(-forward.y, forward.x);
    float half_width = 0.5 * max(line.width, 1.0);
    out.reach = float2(0.5 * span + half_width, half_width);
    // Which end of the quad, the first point's or the second's, and which side.
    float way = corner.position.x < 0.0 ? -1.0 : 1.0;
    float side = corner.position.y < 0.0 ? -1.0 : 1.0;
    float4 position = way < 0.0 ? from : to;
    float2 offset = (forward * way + sideways * side) * (half_width + MARGIN);
    // Moved in clip space by the offset in pixels, at the end's own depth.
    position.xy += offset / half_view * position.w;
    out.position = position;
    out.at = float2(way, side) * (out.reach + MARGIN);
    return out;
}

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Line line = push.params[input.instance];
    // Coverage along and across: all within the reach less half a pixel, none past it plus half.
    float2 covered = saturate(input.reach + 0.5 - abs(input.at));
    float4 color = line.color;
    color.a *= covered.x * covered.y * saturate(line.width);
    // Its ID would hide what's behind from picks.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = color;
    out.id = input.id;
    return out;
}
