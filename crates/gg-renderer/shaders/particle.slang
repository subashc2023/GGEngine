// What the particle shaders share, which particles.slang and emit.slang include: a particle's birth
// record, as the pool holds it, and an emitter's part of a frame, its look, which drawing reads, and
// its emission, which emitting reads too, and the hash that seeds a particle's randomness.
// `GpuLook` and `GpuEmission` in src/particles.rs, which `Look` and `Emission` here match field for
// field.

static const float PI = 3.14159265358979;

// A particle's birth record: where and when it was born, how fast it went, how long it lives and
// how it turns, from which its age alone gives where it is and how it looks, as particles.slang
// finds them.
struct Particle {
    // Where it was born, in the world.
    float3 position;
    // The tick it was born in, as the low 32 bits of the world's ticks.
    uint tick;
    // Its velocity at birth, in the world, as half floats: X in the low half, then Y.
    uint velocity_xy;
    // Its velocity's Z as a half float, then how far into its tick it was born, a 16-bit fraction.
    uint velocity_z_fraction;
    // Its lifetime, in seconds, then its spin, in radians a second, as half floats. A lifetime of 0
    // is no particle, as a slot cleared to zeros holds.
    uint lifetime_spin;
    // Its turn at birth, in radians.
    float angle;
};

// What drawing an emitter's particles reads: how they look by their age, where its ring is in the
// pool, and the world's time as the frame draws it.
struct Look {
    // Their colors at birth and at death, linear RGBA, alpha not multiplied in.
    float4 color_birth;
    float4 color_death;
    // What moves them besides their velocity: a constant acceleration, in the world, and linear
    // drag, the share of its velocity a particle loses a second, of no less than 0.
    float3 acceleration;
    float drag;
    // Their sizes at birth and at death, across, in the world.
    float2 size;
    // The emitter's ID, its low and high halves, which its particles write where they show.
    uint2 id;
    // The cells of their texture across and down, which each plays through over its life, row by
    // row from the top left, each at least 1.
    uint2 cells;
    // Their texture, a texture of the bindless set's, and its sampler; or `DISC` for a soft disc.
    uint texture;
    uint sampler;
    // 1 where they add their light to what's behind them, writing no alpha; 0 where they go over it
    // by their alpha.
    uint additive;
    // The ring's first slot in the pool, and how many it has.
    uint offset;
    uint capacity;
    // The ring's slot that holds its oldest particle, which draws first: the slot its next birth
    // goes to.
    uint drawn_from;
    // The world's time as the frame draws it: the low 32 bits of its ticks, and how far it is into
    // the next.
    uint now_tick;
    float now_fraction;
    // The seconds a tick takes.
    float tick_seconds;
    uint unused;
};

// What emitting an emitter's particles of a frame reads besides its look: where and how they're
// born, and which of its ring's slots they go to.
struct Emission {
    // Where the frame's births are born between: the emitter's transform the frame before, and its
    // transform this frame, from its space to the world's.
    float4x4 from;
    float4x4 to;
    // Where its bursts are born: its transform as the latest tick left it.
    float4x4 burst_at;
    // Its shape's sizes: a sphere's radius in X; a box's half extents; a cone's angle from its axis,
    // in radians, then its base's radius.
    float4 shape;
    // The least and the most of each particle's speed, lifetime and spin, which each takes a share
    // of between them at random.
    float2 speed;
    float2 lifetime;
    float2 spin;
    // The first birth's number among the emitter's, its low and high halves.
    uint2 first;
    // How many births there are, born evenly from the first, `every` ticks apart: the first in tick
    // `birth_tick`, `birth_fraction` into it, `along` of the way from `from` to `to`, each after
    // `step` further.
    uint births;
    uint birth_tick;
    float birth_fraction;
    float every;
    float along;
    float step;
    // How many burst particles there are after the births, from burst particle `burst_skip` of the
    // burst numbered `first_burst` on, `burst_size` to each burst; all born in tick `burst_tick`,
    // `burst_fraction` into it.
    uint bursts;
    uint burst_skip;
    uint first_burst;
    uint burst_size;
    uint burst_tick;
    float burst_fraction;
    // 0 for a point, 1 for a sphere, 2 for a box and 3 for a cone.
    uint shape_kind;
    // The ring's slot, from its first, that the first birth goes to, unless every slot is written.
    uint write_at;
    // 1 where every slot of the ring is written: first the particles kept from its old ring, then
    // the births, then nothing, as a ring made or grown starts.
    uint rebuild;
    // How many particles it keeps, and where: the old ring's first slot in the pool, how many it
    // has, and which of its slots, from its first, holds the first kept.
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

// `Look::texture` for a soft disc, in place of a texture.
static const uint DISC = 0xFFFFFFFFu;

// PCG's hash of `v` (Jarzynski and Olano, "Hash Functions for GPU Rendering", JCGT 2020), which
// seeds a particle's randomness.
uint pcg(uint v) {
    uint state = v * 747796405u + 2891336453u;
    uint word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// A number from 0 up to 1, of 24 of `bits`' bits.
float unit(uint bits) {
    return float(bits >> 8) * (1.0 / 16777216.0);
}

// `value` as a half float's bits, rounded to the nearest, ties to even: what's too small for a
// normal half is 0, and what's too large, infinite or no number is infinite. By 32-bit integers
// rather than `f32tof16`, which slangc makes of 16-bit types, which take device features the engine
// doesn't enable, and which rounds as the device likes.
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
    // Rounding up carries into the exponent as it should, to infinity at the largest.
    uint rest = mantissa & 0x1FFFu;
    if (rest > 0x1000u || (rest == 0x1000u && (half & 1u) != 0u)) {
        half += 1u;
    }
    return half;
}

// The float of a half float's bits, `half_bits`', in the low 16 bits of `bits`.
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
