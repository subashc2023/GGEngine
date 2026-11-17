// What particles.slang and emit.slang share: a particle's birth record as the pool holds it; an
// emitter's part of a frame, its look, which drawing reads, and its emission, which emitting reads
// too; and the hash seeding a particle's randomness. `Look` and `Emission` match `GpuLook` and
// `GpuEmission` in src/particles.rs field for field.

#include "common.slang"

// A particle's birth record, from which its age alone gives where it is and how it looks, as
// particles.slang finds them.
struct Particle {
    // Where it was born, in the world.
    float3 position;
    // Its birth tick, the low 32 bits of the world's ticks.
    uint tick;
    // Its velocity at birth, in the world, as half floats: X in the low half, then Y.
    uint velocity_xy;
    // Its velocity's Z as a half float, then how far into its tick it was born, a 16-bit fraction.
    uint velocity_z_fraction;
    // Its lifetime in seconds, then its spin in radians a second, as half floats. Lifetime 0 is no
    // particle, as a slot cleared to zeros holds.
    uint lifetime_spin;
    // Its turn at birth, in radians.
    float angle;
};

// What drawing an emitter's particles reads: their look by age, the ring's place in the pool, and
// the world's time as the frame draws it.
struct Look {
    // Linear RGBA, alpha not multiplied in.
    float4 color_birth;
    float4 color_death;
    // Besides velocity: a constant acceleration, in the world, and linear drag, the share of its
    // velocity a particle loses a second, at least 0.
    float3 acceleration;
    float drag;
    // At birth and at death, across, in the world.
    float2 size;
    // The emitter's ID, low half then high, which its particles write where they show.
    uint2 id;
    // The texture's cells across and down, each at least 1, which each particle plays through over
    // its life, row by row from the top left.
    uint2 cells;
    // A bindless texture and its sampler, or `DISC` for a soft disc.
    uint texture;
    uint sampler;
    // 1 to add their light, writing no alpha; 0 to go over what's behind by their alpha.
    uint additive;
    // The ring's first slot in the pool, and its length.
    uint offset;
    uint capacity;
    // The ring's slot of its oldest particle, which draws first: where the next birth goes.
    uint drawn_from;
    // The world's time as the frame draws it: the low 32 bits of its ticks, and how far into the
    // next.
    uint now_tick;
    float now_fraction;
    // The seconds a tick takes.
    float tick_seconds;
    uint unused;
};

// What emitting an emitter's particles for a frame reads besides its look: where and how they're
// born, and which ring slots they go to.
struct Emission {
    // The frame's births are born between the emitter's transforms of the frame before and this
    // one, from its space to the world.
    float4x4 from;
    float4x4 to;
    // Where its bursts are born: its transform as the latest tick left it.
    float4x4 burst_at;
    // A sphere's radius in X; a box's half extents; a cone's angle from its axis, in radians, then
    // its base's radius.
    float4 shape;
    // The least and most, which each particle takes a random share between.
    float2 speed;
    float2 lifetime;
    float2 spin;
    // The first birth's number among the emitter's, low half then high.
    uint2 first;
    // Births, evenly from the first, `every` ticks apart. The first is in tick `birth_tick`,
    // `birth_fraction` into it, `along` the way from `from` to `to`; each next `step` further.
    uint births;
    uint birth_tick;
    float birth_fraction;
    float every;
    float along;
    float step;
    // Burst particles after the births: from particle `burst_skip` of burst `first_burst` on,
    // `burst_size` a burst, all born in tick `burst_tick`, `burst_fraction` into it.
    uint bursts;
    uint burst_skip;
    uint first_burst;
    uint burst_size;
    uint burst_tick;
    float burst_fraction;
    // 0 for a point, 1 a sphere, 2 a box, 3 a cone.
    uint shape_kind;
    // The ring slot, from its first, of the first birth, unless every slot is written.
    uint write_at;
    // 1 to write every ring slot, as a ring made or grown starts: the particles kept from the old
    // ring, then the births, then nothing.
    uint rebuild;
    // How many particles it keeps, and where: the old ring's first pool slot, its length, and its
    // slot, from its first, of the first kept.
    uint kept;
    uint old_offset;
    uint old_capacity;
    uint old_first;
    uint unused;
};

// An emitter's part of a frame, its look first, which drawing reads alone.
struct Emitter {
    Look look;
    Emission emission;
};

// `Look::texture` for a soft disc.
static const uint DISC = 0xFFFFFFFFu;

// PCG's hash (Jarzynski and Olano, "Hash Functions for GPU Rendering", JCGT 2020), seeding a
// particle's randomness.
uint pcg(uint v) {
    uint state = v * 747796405u + 2891336453u;
    uint word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// A number in [0, 1) from 24 of `bits`' bits.
float unit(uint bits) {
    return float(bits >> 8) * (1.0 / 16777216.0);
}

// `value` as a half float's bits, rounded to nearest, ties to even. Too small for a normal half is
// 0; too large, infinite or NaN is infinite. By 32-bit integers, not `f32tof16`: slangc makes that
// of 16-bit types, which take device features the engine doesn't enable, and it rounds as the
// device likes.
uint half_bits(float value) {
    uint bits = asuint(value);
    uint sign = (bits >> 16) & 0x8000u;
    int exponent = int((bits >> 23) & 0xFFu) - 112;
    if (exponent <= 0) {
        return sign;
    }
    if (exponent >= 31) {
        return sign | 0x7C00u;
    }
    uint mantissa = bits & 0x7FFFFFu;
    uint half = sign | (uint(exponent) << 10) | (mantissa >> 13);
    // Rounding up carries into the exponent, to infinity at the largest.
    uint rest = mantissa & 0x1FFFu;
    if (rest > 0x1000u || (rest == 0x1000u && (half & 1u) != 0u)) {
        half += 1u;
    }
    return half;
}

// The float of `half_bits`' output, in the low 16 bits of `bits`.
float from_half(uint bits) {
    uint sign = (bits & 0x8000u) << 16;
    uint exponent = (bits >> 10) & 0x1Fu;
    uint mantissa = (bits & 0x3FFu) << 13;
    if (exponent == 0) {
        return asfloat(sign);
    }
    if (exponent == 31) {
        return asfloat(sign | 0x7F800000u | mantissa);
    }
    return asfloat(sign | ((exponent + 112) << 23) | mantissa);
}
