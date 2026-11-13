// Particles' births, written into their emitters' rings in the pool before the frame draws them:
// `emit_main`, a thread for each slot an emitter's frame writes, its births', or, for a ring made or
// grown, every slot of it, the particles kept from its old ring first, then the births, then
// nothing; `grow_main`, a thread for each word of the pool copied into the pool that takes its
// place, a larger one, in the same slots; and `moved_main`, a thread for each slot of the pool,
// moving each particle's birth by `-shift`, as a frame whose origin moved by `shift` does. A
// birth's randomness is a hash of its emitter's ID and its number, so that it's born alike whatever
// the frames. `EmitPush` in src/particles.rs matches the one here byte for byte.

#include "particle.slang"

struct EmitPush {
    // The pool the frame writes, which every emitter's ring is in.
    Particle* pool;
    // The emitter's part of the frame, for `emit_main`.
    Emitter* emitter;
    // The pool before it grew, for `grow_main`, a word at a time.
    uint* from;
    // The slots `emit_main` writes, or the words of the pool before it grew.
    uint count;
    // The first word this dispatch of `grow_main` copies, or slot `moved_main` moves.
    uint first;
    // How far the frame's origin moved, for `moved_main`.
    float3 shift;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<EmitPush> push;

[shader("compute")]
[numthreads(64, 1, 1)]
void emit_main(uint3 thread: SV_DispatchThreadID) {
    uint i = thread.x;
    if (i >= push.count) {
        return;
    }
    Emitter emitter = push.emitter[0];
    Look look = emitter.look;
    Emission emission = emitter.emission;
    uint born = emission.births + emission.bursts;
    uint slot;
    Particle particle;
    if (emission.rebuild != 0) {
        slot = i;
        if (i < emission.kept) {
            uint kept = (emission.old_first + i) % emission.old_capacity;
            particle = push.pool[emission.old_offset + kept];
        } else if (i < emission.kept + born) {
            particle = birth(look, emission, i - emission.kept);
        } else {
            particle = none();
        }
    } else {
        slot = (emission.write_at + i) % look.capacity;
        particle = birth(look, emission, i);
    }
    // A field at a time, as slangc leaves the alignment off a store of a whole struct copied from
    // an address, which SPIR-V requires.
    uint at = look.offset + slot;
    push.pool[at].position = particle.position;
    push.pool[at].tick = particle.tick;
    push.pool[at].velocity_xy = particle.velocity_xy;
    push.pool[at].velocity_z_fraction = particle.velocity_z_fraction;
    push.pool[at].lifetime_spin = particle.lifetime_spin;
    push.pool[at].angle = particle.angle;
}

[shader("compute")]
[numthreads(64, 1, 1)]
void grow_main(uint3 thread: SV_DispatchThreadID) {
    uint word = push.first + thread.x;
    if (word >= push.count) {
        return;
    }
    ((uint*)push.pool)[word] = push.from[word];
}

[shader("compute")]
[numthreads(64, 1, 1)]
void moved_main(uint3 thread: SV_DispatchThreadID) {
    uint slot = push.first + thread.x;
    if (slot >= push.count) {
        return;
    }
    push.pool[slot].position = push.pool[slot].position - push.shift;
}

// Birth `i` of the emitter's frame: its births, then its burst particles.
Particle birth(Look look, Emission emission, uint i) {
    uint seed;
    uint tick;
    float fraction;
    float4x4 at;
    if (i < emission.births) {
        // Its number among the emitter's, 64 bits as two halves.
        uint low = emission.first.x + i;
        uint high = emission.first.y + (low < i ? 1u : 0u);
        seed = pcg(look.id.x ^ pcg(look.id.y ^ pcg(low ^ pcg(high))));
        float into = emission.birth_fraction + float(i) * emission.every;
        float whole = floor(into);
        tick = emission.birth_tick + uint(whole);
        fraction = into - whole;
        float along = saturate(emission.along + float(i) * emission.step);
        at = emission.from + (emission.to - emission.from) * along;
    } else {
        uint q = emission.burst_skip + (i - emission.births);
        uint burst = emission.first_burst + q / emission.burst_size;
        uint within = q % emission.burst_size;
        seed = pcg(look.id.x ^ pcg(look.id.y ^ pcg(burst ^ pcg(within ^ 0x9E3779B9u))));
        tick = emission.burst_tick;
        fraction = emission.burst_fraction;
        at = emission.burst_at;
    }
    float r[8];
    for (uint k = 0; k < 8; k++) {
        r[k] = unit(pcg(seed + k));
    }
    float3 position = float3(0.0);
    float3 direction = float3(0.0, 1.0, 0.0);
    if (emission.shape_kind == 0) {
        direction = any_way(r[0], r[1]);
    } else if (emission.shape_kind == 1) {
        direction = any_way(r[0], r[1]);
        position = direction * emission.shape.x * pow(r[2], 1.0 / 3.0);
    } else if (emission.shape_kind == 2) {
        position = (float3(r[0], r[1], r[2]) * 2.0 - 1.0) * emission.shape.xyz;
    } else {
        // Uniformly over the cone's cap of directions, from a disc at its base.
        float across = emission.shape.y * sqrt(r[0]);
        float about = 2.0 * PI * r[1];
        position = float3(across * cos(about), 0.0, across * sin(about));
        float cosine = 1.0 - r[2] * (1.0 - cos(emission.shape.x));
        float sine = sqrt(max(0.0, 1.0 - cosine * cosine));
        float turn = 2.0 * PI * r[3];
        direction = float3(sine * cos(turn), cosine, sine * sin(turn));
    }
    float3 world = mul(at, float4(position, 1.0)).xyz;
    float3 way = mul(at, float4(direction, 0.0)).xyz;
    float reach = sqrt(dot(way, way));
    // An emitter scaled to nothing sends its particles nowhere.
    way = reach > 0.0 ? way / reach : float3(0.0);
    float3 velocity = way * lerp(emission.speed.x, emission.speed.y, r[4]);
    float lifetime = max(lerp(emission.lifetime.x, emission.lifetime.y, r[5]), 0.0);
    float spin = lerp(emission.spin.x, emission.spin.y, r[6]);
    uint into = uint(round(saturate(fraction) * 65535.0));
    Particle particle;
    particle.position = world;
    particle.tick = tick;
    particle.velocity_xy = half_bits(velocity.x) | (half_bits(velocity.y) << 16);
    particle.velocity_z_fraction = half_bits(velocity.z) | (into << 16);
    particle.lifetime_spin = half_bits(lifetime) | (half_bits(spin) << 16);
    particle.angle = 2.0 * PI * r[7];
    return particle;
}

// A way uniformly over the sphere's, of `u` and `v` from 0 up to 1.
float3 any_way(float u, float v) {
    float y = 1.0 - 2.0 * u;
    float across = sqrt(max(0.0, 1.0 - y * y));
    float about = 2.0 * PI * v;
    return float3(across * cos(about), y, across * sin(about));
}

// No particle: a lifetime of 0, as a slot cleared to zeros holds.
Particle none() {
    Particle particle;
    particle.position = float3(0.0);
    particle.tick = 0;
    particle.velocity_xy = 0;
    particle.velocity_z_fraction = 0;
    particle.lifetime_spin = 0;
    particle.angle = 0.0;
    return particle;
}
