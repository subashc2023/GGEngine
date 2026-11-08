// Particles: an instance of the engine's quad for each slot of an emitter's ring, from its oldest
// particle's, facing the view, each where its birth record and its age put it: carried by its
// velocity, constant acceleration and linear drag, in closed form, sized, colored, turned and on the
// cell of its texture its share of its life says, so that it draws alike whatever the frames between
// its birth and now. A slot whose particle isn't born yet, or has died, draws nothing. Each shows its
// texture, or a soft disc, tinted, and goes over what's behind it by its alpha, premultiplied, or
// adds its light, writing no alpha, its emitter's ID where it shows but where it adds next to
// nothing. `DrawPush` in src/particles.rs matches the one here byte for byte.

#include "mesh.slang"
#include "particle.slang"

struct DrawPush {
    // From the world to clip space.
    float4x4 clip_from_world;
    // The pool its emitter's ring is in, and its emitter's look.
    Particle* pool;
    Look* look;
    // The ways right and up across the view, in the world, which a particle's quad faces along.
    float3 right;
    float unused_right;
    float3 up;
    float unused_up;
};
[[vk::push_constant]] ConstantBuffer<DrawPush> push;

struct Varyings {
    float4 position : SV_Position;
    // Where on its texture, in texture coordinates.
    float2 uv;
    // Where on its quad, from -1 to 1 each way, for the disc.
    float2 disc;
    // Its color, linear RGBA, alpha not multiplied in.
    nointerpolation float4 color;
    // Where its texture may be sampled, in texture coordinates: the least corner, then the most,
    // so that a linear sampler reaches no texel of the next cell.
    nointerpolation float4 inside;
    nointerpolation uint2 id;
    nointerpolation uint texture;
    nointerpolation uint sampler;
    nointerpolation uint additive;
};

// The quad's corners, as its indices draw them, counterclockwise from its bottom left.
static const float2 CORNERS[4] = {
    float2(-0.5, -0.5),
    float2(0.5, -0.5),
    float2(0.5, 0.5),
    float2(-0.5, 0.5),
};

[shader("vertex")]
Varyings vertex_main(uint vertex: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Look look = push.look[0];
    Particle particle = push.pool[look.offset + (look.drawn_from + instance) % look.capacity];
    float born = float(particle.velocity_z_fraction >> 16) / 65535.0;
    // In whole ticks, by 32-bit integers, which wrap alike, then the fractions: exact however long
    // the world has run.
    float ticks = float(look.now_tick - particle.tick) + (look.now_fraction - born);
    float age = ticks * look.tick_seconds;
    float lifetime = from_half(particle.lifetime_spin);
    Varyings out;
    out.uv = float2(0.0);
    out.disc = float2(0.0);
    out.color = float4(0.0);
    out.inside = float4(0.0);
    out.id = look.id;
    out.texture = look.texture;
    out.sampler = look.sampler;
    out.additive = look.additive;
    // Not born yet, or dead: each corner at one point past the far plane, which draws nothing.
    if (!(age >= 0.0 && age < lifetime)) {
        out.position = float4(0.0, 0.0, -1.0, 1.0);
        return out;
    }
    float3 velocity = float3(from_half(particle.velocity_xy), from_half(particle.velocity_xy >> 16),
                             from_half(particle.velocity_z_fraction));
    float spin = from_half(particle.lifetime_spin >> 16);
    float share = age / lifetime;
    float3 position = particle.position + moved(velocity, look.acceleration, look.drag, age);
    float size = lerp(look.size.x, look.size.y, share);
    float turn = particle.angle + spin * age;
    float2 corner = CORNERS[vertex];
    float2 turned = float2(cos(turn) * corner.x - sin(turn) * corner.y,
                           sin(turn) * corner.x + cos(turn) * corner.y) * size;
    float3 world = position + push.right * turned.x + push.up * turned.y;
    out.position = mul(push.clip_from_world, float4(world, 1.0));
    out.disc = corner * 2.0;
    out.color = lerp(look.color_birth, look.color_death, share);
    // The cell its share of its life is on, row by row from the top left.
    uint cells = look.cells.x * look.cells.y;
    uint cell = min(uint(share * float(cells)), cells - 1);
    float2 across = 1.0 / float2(look.cells);
    float2 first = float2(cell % look.cells.x, cell / look.cells.x) * across;
    out.uv = first + float2(corner.x + 0.5, 0.5 - corner.y) * across;
    if (look.texture != DISC) {
        // Half a texel inside the cell's edges, as a linear sampler reads the 2 by 2 texels around
        // a point.
        uint width, height;
        textures[NonUniformResourceIndex(look.texture)].GetDimensions(width, height);
        float2 texel = 0.5 / float2(width, height);
        float2 least = min(first + texel, first + across * 0.5);
        float2 most = max(first + across - texel, first + across * 0.5);
        out.inside = float4(least, most);
    }
    return out;
}

[shader("fragment")]
Targets fragment_main(Varyings input) {
    float4 color = input.color;
    if (input.texture == DISC) {
        // Brightest at its middle, falling smoothly to nothing at its edge.
        float near = saturate(1.0 - dot(input.disc, input.disc));
        color.a *= near * near;
    } else {
        // Decoded from sRGB to linear as it's sampled, as the view's colors are.
        Texture2D texture = textures[NonUniformResourceIndex(input.texture)];
        SamplerState sampler = samplers[NonUniformResourceIndex(input.sampler)];
        color *= texture.Sample(sampler, clamp(input.uv, input.inside.xy, input.inside.zw));
    }
    float alpha = saturate(color.a);
    float3 premultiplied = max(color.rgb, float3(0.0)) * alpha;
    // What adds next to nothing, or covers next to nothing, changes no pixel, but its ID would hide
    // what's behind it from picks. It writes no depth, so it keeps early depth testing.
    float shown = input.additive != 0 ? max(premultiplied.r, max(premultiplied.g, premultiplied.b))
                                      : alpha;
    if (!(shown >= 1.0 / 1024.0)) {
        discard;
    }
    Targets out;
    out.color = float4(premultiplied, input.additive != 0 ? 0.0 : alpha);
    out.id = input.id;
    return out;
}

// How far a particle of velocity `v` at birth has gone `t` seconds after it, under constant
// acceleration `a` and linear drag `k`, of no less than 0: of dv/dt = a - kv, v's integral,
// v(1 - e^-kt)/k + a(t - (1 - e^-kt)/k)/k, by series where kt is small and the divisions would
// cancel, which tends to vt + at²/2 as k does.
float3 moved(float3 v, float3 a, float k, float t) {
    float kt = k * t;
    float carried;
    float pulled;
    if (kt < 0.01) {
        carried = t * (1.0 - kt * (0.5 - kt / 6.0));
        pulled = t * t * (0.5 - kt * (1.0 / 6.0 - kt / 24.0));
    } else {
        carried = (1.0 - exp(-kt)) / k;
        pulled = (t - carried) / k;
    }
    return v * carried + a * pulled;
}
