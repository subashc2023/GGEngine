// Lines: from one point to another in a draw's space, on the engine's quad, which the vertex stage
// lays along the line as the view shows it and widens across it to the line's width in the view's
// pixels, however near or far it is, its ends squared off half its width past its points, so that
// a rectangle's sides meet at its corners. The fragment stage smooths its edges over a pixel; a
// line thinner than a pixel draws a pixel wide, as much fainter. A line is cut where it crosses the
// near plane, since what's behind the camera would show mirrored through it. Each instance is a
// line, with its own transform, ID and parameters, drawn by its alpha over what's behind it.
// `Params` in src/line.rs, which `Line` here matches byte for byte.

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
    // Where it is from the middle of the line as the view shows it, in pixels: along the line, from
    // its first point toward its second, then across it; straight across the screen.
    noperspective float2 at;
    // How far the line reaches from its middle along it and across it, in pixels, to where it
    // covers half a pixel.
    nointerpolation float2 reach;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

// How much further than the line's reach the quad goes, in pixels, so that its smoothed edges fit
// inside it.
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
    // In front of the near plane, where depth runs down from 1, z is at most w. A line wholly
    // behind it draws nothing: every corner is outside the view.
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
    // Seen end on, it has no way along: any will do, for a square as wide as it.
    float2 forward = span > 1e-6 ? along / span : float2(1.0, 0.0);
    float2 sideways = float2(-forward.y, forward.x);
    float half_width = 0.5 * max(line.width, 1.0);
    out.reach = float2(0.5 * span + half_width, half_width);
    // The first point's end of the quad or the second's, and the line's side it's on.
    float way = corner.position.x < 0.0 ? -1.0 : 1.0;
    float side = corner.position.y < 0.0 ? -1.0 : 1.0;
    float4 position = way < 0.0 ? from : to;
    float2 offset = (forward * way + sideways * side) * (half_width + MARGIN);
    // Moved in clip space as far as the offset in pixels, at the end's own depth.
    position.xy += offset / half_view * position.w;
    out.position = position;
    out.at = float2(way, side) * (out.reach + MARGIN);
    return out;
}

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Line line = push.params[input.instance];
    // How much of the pixel the line covers, along it and across it: all of it inside its reach
    // less half a pixel, none outside it plus half a pixel. A line thinner than a pixel draws a
    // pixel wide, as much fainter as it's thinner.
    float2 covered = saturate(input.reach + 0.5 - abs(input.at));
    float4 color = line.color;
    color.a *= covered.x * covered.y * saturate(line.width);
    // Blending what's fully transparent changes no pixel, but its ID would hide what's behind it
    // from picks.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = color;
    out.id = input.id;
    return out;
}
