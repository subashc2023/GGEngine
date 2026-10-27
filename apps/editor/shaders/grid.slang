// The editor's grid: lines a power of ten apart on one of the world's axis planes, XY, XZ or YZ,
// and the plane's two axes in their colors, seen through either projection. The mesh is a unit
// square about its center, `SQUARE` in src/grid.rs, which its transform lays in the plane: around
// the camera's foot in perspective, and over the view orthographically. `Params` there matches
// `Grid` here byte for byte.
//
// Lines a power of ten apart show once they're 8 pixels apart, fading in until they're 80 apart,
// and brighten from there to the major lines' gray by 800. In an orthographic view that faces the
// plane squarely they're crisp, a pixel each; in any other they're anti-aliased, each family of
// lines by its own footprint, and fade toward grazing angles.

// How the lines draw, which `Grid.lines` says: anti-aliased, seen through perspective from a point
// or orthographically along a direction; or crisp, orthographically, facing the plane squarely.
static const uint PERSPECTIVE = 0;
static const uint ORTHOGRAPHIC = 1;
static const uint CRISP = 2;

struct Grid {
    // The color of the plane's first axis, linear: X in XY and XZ, Y in YZ.
    float3 first;
    // Where the quad's center is along the first axis, modulo the coarsest spacing drawn.
    float first_center;
    // The color of its second axis: Y in XY, Z in XZ and YZ.
    float3 second;
    float second_center;
    // In perspective, where the camera is from the quad's center; orthographically, the way it
    // faces.
    float3 eye;
    // The power of ten that the coarsest spacing drawn is.
    float coarsest;
    // The plane's normal: Z, Y or X.
    float3 normal;
    uint lines;
};

struct Push {
    float4x4 clip_from_world;
    float2* corners;
    // Each instance's, from the square's space to the world's.
    float4x4* transforms;
    Grid* grids;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    // Where the fragment is on the plane: its offset from the quad's center, plus where the center
    // is modulo the coarsest spacing drawn, which every finer spacing divides. So the lines are
    // where they'd be from the plane's origin, and as precise however far from it they are.
    float2 plane;
    // Where it is on the plane from the plane's origin, for the axes through it: what rounds off
    // where the center is far from the origin is far less than a pixel wherever an axis shows,
    // which is as far off.
    float2 axes;
    nointerpolation uint instance;
};

// How far apart, in pixels, the finest lines that show are at least.
static const float FINEST = 8.0;
// The lines' color, linear, and how much of it minor and major lines take over what's behind them.
static const float3 GRAY = float3(0.3, 0.3, 0.3);
static const float MINOR = 0.35;
static const float MAJOR = 1.0;
// How squarely, as |n·v|, the camera sees the plane where anti-aliased lines have faded out, and
// where they begin to: at grazing angles, where the quad's edge is in perspective.
static const float GRAZING = 0.002;
static const float GRAZING_FADE = 0.02;

// The plane's coordinates of `v`, a point or an offset in the world, along the axes its name
// gives in order, for the plane whose normal is `normal`.
float2 in_plane(float3 v, float3 normal) {
    if (normal.x != 0.0) {
        return v.yz;
    }
    if (normal.y != 0.0) {
        return v.xz;
    }
    return v.xy;
}

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    float4x4 transform = push.transforms[instance];
    Grid grid = push.grids[instance];
    // The quad's center, and the corner's offset from it along the quad's edges, taken through the
    // matrix apart: rounding where the camera and the center are far from the origin then moves
    // every corner alike, which keeps the quad flat.
    float3 center = float3(transform[0][3], transform[1][3], transform[2][3]);
    float3 offset = mul(transform, float4(push.corners[index], 0.0, 0.0)).xyz;
    float4x4 m = push.clip_from_world;
    Varyings out;
    out.position = mul(m, float4(center, 1.0)) + mul(m, float4(offset, 0.0));
    float2 along = in_plane(offset, grid.normal);
    out.plane = float2(grid.first_center, grid.second_center) + along;
    out.axes = in_plane(center, grid.normal) + along;
    out.instance = instance;
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

// Crisp lines, a pixel each, for an orthographic view facing the plane squarely, where `pixel` is
// the same across both axes.
float4 crisp_lines(float2 at, float2 axes, float2 pixel, Grid grid) {
    // The power of ten whose lines are the finest that show, at least FINEST pixels apart, and how
    // far they are from FINEST apart toward ten times it, which they fade in over.
    float finest = log10(max(pixel.x, pixel.y) * FINEST);
    float level = ceil(finest);
    float fade = level - finest;
    float spacing = pow(10.0, level);
    // A coarser line is a finer one too, and takes its own weight, which is more.
    float weight = 0.0;
    if (on_lines(at, pixel, spacing)) {
        weight = MINOR * fade;
    }
    if (on_lines(at, pixel, spacing * 10.0)) {
        weight = lerp(MINOR, MAJOR, fade);
    }
    if (on_lines(at, pixel, spacing * 100.0)) {
        weight = MAJOR;
    }
    // The axes over the lines, the second over the first: the second lies where the first
    // coordinate is 0.
    if (on_axis(axes.x, pixel.x)) {
        return float4(grid.second, 1.0);
    }
    if (on_axis(axes.y, pixel.y)) {
        return float4(grid.first, 1.0);
    }
    return float4(GRAY, weight);
}

// How much of the pixel the lines at each multiple of `spacing` cover, where the coordinate across
// them is `at` and changes by `pixel` from one pixel to the next: Ben Golus's pristine grid, from
// "The Best Darn Grid Shader (Yet)" (2023),
// https://bgolus.medium.com/the-best-darn-grid-shader-yet-727f9278b9d8, for lines a pixel wide. A
// line is drawn no wider than half the space between lines, its edges ramped over a pixel and a
// half, and lines crowding closer than two pixels apart fade to the share of the pixel they cover
// on average, so that they never alias into moiré. Lines a pixel wide cover less than half the
// space between them while they're more than two pixels apart, so his case for lines that cover
// more is left out.
float coverage(float at, float pixel, float spacing) {
    // The spaces between lines that a pixel spans, and so how wide a line a pixel wide is in them.
    float cells = pixel / spacing;
    // Twice the distance to the nearest line, in those spaces.
    float across = 1.0 - abs(frac(at / spacing) * 2.0 - 1.0);
    float width = min(cells, 0.5);
    float drawn = 1.0 - smoothstep(width - cells * 1.5, width + cells * 1.5, across);
    return lerp(drawn, min(cells, 1.0), saturate(cells * 2.0 - 1.0));
}

// How much of the pixel a family of lines covers, the lines across one coordinate, which is `at`
// and changes by `pixel` from one pixel to the next: each power of ten's lines weighed as
// `crisp_lines` weighs them, the coarser over the finer, but anti-aliased, and none coarser than
// the power of ten `coarsest`, which the plane's coordinates are taken modulo.
float family(float at, float pixel, float coarsest) {
    float finest = log10(pixel * FINEST);
    float level = ceil(finest);
    float fade = level - finest;
    float spacing = pow(10.0, level);
    float weight = 0.0;
    if (level <= coarsest) {
        weight = MINOR * fade * coverage(at, pixel, spacing);
    }
    if (level + 1.0 <= coarsest) {
        weight = max(weight, lerp(MINOR, MAJOR, fade) * coverage(at, pixel, spacing * 10.0));
    }
    if (level + 2.0 <= coarsest) {
        weight = max(weight, MAJOR * coverage(at, pixel, spacing * 100.0));
    }
    return weight;
}

// How much of the pixel the axis at 0 covers, where the coordinate across it is `at` and changes
// by `pixel` from one pixel to the next, as `coverage` draws a line.
float axis(float at, float pixel) {
    return 1.0 - smoothstep(-0.5 * pixel, 2.5 * pixel, 2.0 * abs(at));
}

// Anti-aliased lines, for a view with perspective, or orthographic but not facing the plane
// squarely.
float4 smooth_lines(float2 at, float2 axes, float2 pixel, Grid grid) {
    // A footprint no less than the coordinates resolve where they're largest, about a ten
    // millionth of the coarsest spacing: finer lines than that are past a float's precision.
    pixel = max(pixel, pow(10.0, grid.coarsest - 7.0));
    float gray = max(family(at.x, pixel.x, grid.coarsest), family(at.y, pixel.y, grid.coarsest));
    // The lines, then the first axis over them, then the second, as `crisp_lines` draws them: the
    // color multiplied by its alpha until the end.
    float4 color = float4(GRAY * gray, gray);
    color = lerp(color, float4(grid.first, 1.0), axis(axes.y, pixel.y));
    color = lerp(color, float4(grid.second, 1.0), axis(axes.x, pixel.x));
    // How squarely the camera sees the fragment's place on the plane, |n·v|: the same everywhere
    // orthographically, and in perspective its height above the plane over its distance.
    float height = dot(grid.eye, grid.normal);
    float facing = abs(height);
    if (grid.lines == PERSPECTIVE) {
        float2 center = float2(grid.first_center, grid.second_center);
        float2 from = at - center - in_plane(grid.eye, grid.normal);
        facing /= length(float3(from, height));
    }
    float alpha = color.a * smoothstep(GRAZING, GRAZING_FADE, facing);
    return float4(color.rgb / max(color.a, 1e-6), alpha);
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    Grid grid = push.grids[input.instance];
    // The plane units each coordinate changes by from one pixel to the next, the footprint across
    // each family of lines: the length of the coordinate's derivatives, as Ben Golus's grid takes
    // it, rather than fwidth's sum of them, so that lines keep their width whichever way they run.
    float2 dx = ddx(input.plane);
    float2 dy = ddy(input.plane);
    float2 pixel = float2(length(float2(dx.x, dy.x)), length(float2(dx.y, dy.y)));
    if (grid.lines == CRISP) {
        return crisp_lines(input.plane, input.axes, pixel, grid);
    }
    return smooth_lines(input.plane, input.axes, pixel, grid);
}
