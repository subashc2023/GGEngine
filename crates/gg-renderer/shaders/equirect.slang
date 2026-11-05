// What sky.slang and prefilter.slang share: where a way in the world falls in an equirectangular
// image, as Khronos's glTF Sample Renderer and three.js map one, its center column toward +X, a
// quarter of the way across toward -Z, and its top row straight up; and a texel of a cube's face
// as the way it stands for, by Vulkan's table of cube map faces.

static const float PI = 3.14159265358979;
// `Sampler::LinearClamp`'s and `Sampler::LinearRepeat`'s indices in the bindless set.
static const uint LINEAR_CLAMP = 0;
static const uint LINEAR_REPEAT = 2;

// The color `image`, `size` texels across and down, holds the way `direction`, a unit vector,
// from its first level, filtered: across its left and right edges, which meet, and not across its
// top and bottom, which don't.
float3 equirect(Texture2D image, SamplerState repeat, float2 size, float3 direction) {
    float u = 0.5 + atan2(direction.z, direction.x) / (2.0 * PI);
    float v = acos(clamp(direction.y, -1.0, 1.0)) / PI;
    float edge = 0.5 / size.y;
    return image.SampleLevel(repeat, float2(u, clamp(v, edge, 1.0 - edge)), 0.0).rgb;
}

// The way the point `texel` of face `face` of a cube, `size` texels a side, stands for, in texels
// from the face's top left, a texel's center half a texel in, as Vulkan picks a face and a point by
// a way: the faces +X, -X, +Y, -Y, +Z, then -Z.
float3 cube_direction(uint face, float2 texel, float size) {
    float2 st = texel / size * 2.0 - 1.0;
    float3 direction;
    switch (face) {
    case 0: direction = float3(1.0, -st.y, -st.x); break;
    case 1: direction = float3(-1.0, -st.y, st.x); break;
    case 2: direction = float3(st.x, 1.0, st.y); break;
    case 3: direction = float3(st.x, -1.0, -st.y); break;
    case 4: direction = float3(st.x, -st.y, 1.0); break;
    default: direction = float3(-st.x, -st.y, -1.0); break;
    }
    return normalize(direction);
}
