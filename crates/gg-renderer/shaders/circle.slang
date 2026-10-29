// Circles: discs and rings on the engine's quad, each as large as fits inside it, a unit across,
// filled with its color and drawn by its alpha over what's behind it. Each instance is a circle,
// with its own transform, ID and parameters, writing its ID where it shows but where it's fully
// transparent, as past its edge, so that a pick there finds what's behind it. Its edges are smoothed
// over a pixel however big it shows, and faded further inward by its fade. `Corner` in
// src/sprite.rs, which `Corner` here matches field for field, and `Params` in src/circle.rs, which
// `Circle` here matches byte for byte.

struct Corner {
    float3 position;
    float2 uv;
};

struct Circle {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // How far in from its edge it's filled, as a fraction of its radius: 1 or more, all of it.
    float thickness;
    // How far in from each of its edges it fades, as a fraction of its radius.
    float fade;
};

struct Push {
    float4x4 clip_from_world;
    Corner* corners;
    // Each instance's, from the quad's space to the world's.
    float4x4* transforms;
    // Each instance's: its entity's, or 0.
    uint2* ids;
    Circle* circles;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    // Where it is on the quad, from (-1, -1) to (1, 1), so that the circle's edge is 1 from its
    // middle.
    float2 local;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Corner corner = push.corners[index];
    float4 world = mul(push.transforms[instance], float4(corner.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.local = corner.position.xy * 2.0;
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Circle circle = push.circles[input.instance];
    // How far in from its edge, as a fraction of its radius: 0 on the edge, 1 in the middle.
    float inside = 1.0 - length(input.local);
    // How far that goes from one pixel to the next, which the edges are smoothed over, so that
    // they're as smooth at any size: never 0, which would leave a step of no width.
    float pixel = max(length(float2(ddx(inside), ddy(inside))), 1e-6);
    // Half a pixel either side of its edge, and the fade on inward; and as far in as it's thick,
    // for a ring.
    float alpha = smoothstep(-0.5 * pixel, circle.fade + 0.5 * pixel, inside);
    if (circle.thickness < 1.0) {
        float from = circle.thickness - 0.5 * pixel;
        alpha *= 1.0 - smoothstep(from, circle.thickness + circle.fade + 0.5 * pixel, inside);
    }
    float4 color = circle.color;
    color.a *= alpha;
    // Blending what's fully transparent changes no pixel, but its ID would hide what's behind it
    // from picks, as at the quad's corners.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = color;
    out.id = input.id;
    return out;
}
