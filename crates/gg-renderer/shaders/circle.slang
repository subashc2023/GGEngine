// Circles: discs and rings on the engine's quad, each the largest that fits, a unit across, drawn
// by its alpha. An instance per circle, with its own transform, ID and parameters; fully
// transparent fragments write no ID, so picks find what's behind. Edges are smoothed over a pixel
// at any size, then faded inward by `fade`. Fogged at its corners, as fog.slang says. `Circle`
// matches `Params` in src/circle.rs byte for byte.

#include "mesh.slang"
#include "fog.slang"

struct Circle {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // How far in from its edge it's filled, a fraction of its radius: 1 or more fills it.
    float thickness;
    // How far in from each edge it fades, a fraction of its radius.
    float fade;
};

[[vk::push_constant]] ConstantBuffer<Push<Corner, Circle>> push;

struct Varyings {
    float4 position : SV_Position;
    // The fog at its corners, from fog.slang's `fog_at`.
    float4 fog;
    // On the quad, (-1, -1) to (1, 1): the edge is 1 from the middle.
    float2 local;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Corner corner = push.vertices[index];
    float4 world = mul(push.transforms[instance], float4(corner.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    Frame frame = *push.frame;
    out.fog = fog_at(frame, world.xyz);
    out.local = corner.position.xy * 2.0;
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Circle circle = push.params[input.instance];
    // How far in from the edge, a fraction of the radius: 0 on the edge, 1 in the middle.
    float inside = 1.0 - length(input.local);
    // Its change per pixel, which edges are smoothed over. Never 0, a step of no width.
    float pixel = max(length(float2(ddx(inside), ddy(inside))), 1e-6);
    // Half a pixel either side of the edge, then the fade inward; a ring ends `thickness` in.
    float alpha = smoothstep(-0.5 * pixel, circle.fade + 0.5 * pixel, inside);
    if (circle.thickness < 1.0) {
        float from = circle.thickness - 0.5 * pixel;
        alpha *= 1.0 - smoothstep(from, circle.thickness + circle.fade + 0.5 * pixel, inside);
    }
    float4 color = circle.color;
    color.a *= alpha;
    // Its ID would hide what's behind from picks, as at the quad's corners.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = fogged(input.fog, color);
    out.id = input.id;
    return out;
}
