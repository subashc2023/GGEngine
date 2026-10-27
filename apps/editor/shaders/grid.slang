// The editor's grid: lines on the plane z = 0, each one pixel wide, seen through an orthographic
// camera however it's placed or zoomed. The mesh is the view's corners in clip space, `CORNERS` in
// main.rs, which the vertex stage finds on the plane, so that the grid covers the view whatever its
// size. Lines a power of ten apart show once they're 8 pixels apart, fading in until they're 80
// apart, and brighten from there to the major lines' gray by 800; the X axis is red, and the Y
// axis green.

// What the engine pushes for each instanced draw: the matrix from the world to clip space, then the
// address of the mesh's vertices. It reads no transforms or parameters, so it leaves out their
// addresses.
struct Push {
    float4x4 clip_from_world;
    float2* corners;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    // Where the fragment is on the plane, in world units.
    float2 world;
};

// How far apart, in pixels, the finest lines that show are at least.
static const float FINEST = 8.0;
// The lines' color, linear, and how much of it minor and major lines take over what's behind them.
static const float3 GRAY = float3(0.3, 0.3, 0.3);
static const float MINOR = 0.35;
static const float MAJOR = 1.0;
static const float3 RED = float3(0.8, 0.1, 0.1);
static const float3 GREEN = float3(0.1, 0.7, 0.1);

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID) {
    // An orthographic camera's clip space is the world's, moved, turned and scaled, without the
    // divide that perspective takes, so on the plane z = 0, clip.xy = mul(a, world.xy) + b.
    float4x4 m = push.clip_from_world;
    float2x2 a = float2x2(m[0][0], m[0][1], m[1][0], m[1][1]);
    float2 b = float2(m[0][3], m[1][3]);
    float2x2 world_from_clip = float2x2(a[1][1], -a[0][1], -a[1][0], a[0][0]) / determinant(a);
    float2 world = mul(world_from_clip, push.corners[index] - b);
    Varyings out;
    // Through the matrix again, for the plane's depth.
    out.position = mul(m, float4(world, 0.0, 1.0));
    out.world = world;
    return out;
}

// Whether the pixel whose center is `at` along an axis, `pixel` world units across, draws the line
// at 0 on it: the one pixel whose center is up to three quarters of a pixel short of the line, or
// less than a quarter past it. So the line is one pixel wide, and one on a pixel's center or edge,
// as the axes are while the camera is over the origin, is a quarter of a pixel from where rounding
// could tip it into the next.
bool on_axis(float at, float pixel) {
    float past = at + 0.75 * pixel;
    return past >= 0.0 && past < pixel;
}

// Whether that pixel draws a line at a multiple of `spacing` along either axis, as `on_axis` draws
// the axes.
bool on_lines(float2 at, float2 pixel, float spacing) {
    float2 past = frac((at + 0.75 * pixel) / spacing) * spacing;
    return past.x < pixel.x || past.y < pixel.y;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    float2 world = input.world;
    // The world units a pixel spans across each axis's lines: how far that coordinate changes from
    // one pixel to the next, so that lines stay a pixel wide with the camera turned too.
    float2 pixel = float2(length(float2(ddx(world.x), ddy(world.x))),
                          length(float2(ddx(world.y), ddy(world.y))));
    // The power of ten whose lines are the finest that show, at least FINEST pixels apart, and how
    // far they are from FINEST apart toward ten times it, which they fade in over.
    float finest = log10(max(pixel.x, pixel.y) * FINEST);
    float level = ceil(finest);
    float fade = level - finest;
    float spacing = pow(10.0, level);
    // A coarser line is a finer one too, and takes its own weight, which is more.
    float weight = 0.0;
    if (on_lines(world, pixel, spacing)) {
        weight = MINOR * fade;
    }
    if (on_lines(world, pixel, spacing * 10.0)) {
        weight = lerp(MINOR, MAJOR, fade);
    }
    if (on_lines(world, pixel, spacing * 100.0)) {
        weight = MAJOR;
    }
    // The axes over the lines.
    if (on_axis(world.x, pixel.x)) {
        return float4(GREEN, 1.0);
    }
    if (on_axis(world.y, pixel.y)) {
        return float4(RED, 1.0);
    }
    return float4(GRAY, weight);
}
