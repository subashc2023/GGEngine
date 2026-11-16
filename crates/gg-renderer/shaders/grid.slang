// The engine's grid: power-of-ten lines and the two axes, in their colors, on a world axis plane,
// in either projection. The mesh, `SQUARE` in src/grid.rs, is a unit square its transform lays in
// the plane: round the camera's foot in perspective, over the view orthographically. `Grid`
// matches `Params` there byte for byte.
//
// Lines show once 8 pixels apart, fade in by 80 and brighten to the major gray by 800. Crisp, a
// pixel each, in an orthographic view facing the plane squarely; elsewhere anti-aliased per family
// and faded at grazing angles.
//
// A draw of no entity: the pipeline doesn't write its 0 ID, so a pick on a line finds what's under
// it. Fully transparent fragments are discarded.

// `Grid.lines`: anti-aliased in perspective or orthographic; crisp, orthographic facing the plane
// squarely.
static const uint PERSPECTIVE = 0;
static const uint ORTHOGRAPHIC = 1;
static const uint CRISP = 2;

struct Grid {
    // The first axis's linear color: X in XY and XZ, Y in YZ.
    float3 first;
    // The quad's center along the first axis, modulo the coarsest spacing drawn.
    float first_center;
    // The second's: Y in XY, Z in XZ and YZ.
    float3 second;
    float second_center;
    // In perspective, the camera from the quad's center; orthographically, the way it faces.
    float3 eye;
    // The coarsest spacing drawn, as a power of ten.
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
    // Unread: the grid is of no entity.
    uint2* ids;
    Grid* grids;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    // The offset from the quad's center plus the center modulo the coarsest spacing, which every
    // finer one divides: lines fall as from the origin, as precise at any distance.
    float2 plane;
    // From the plane's origin, for the axes. Rounding far from the origin is far less than a pixel
    // wherever an axis shows, which is as far off.
    float2 axes;
    nointerpolation uint instance;
};

// The finest lines' least spacing, in pixels.
static const float FINEST = 8.0;
// The lines' linear color, and the alpha of minor and major lines.
static const float3 GRAY = float3(0.3, 0.3, 0.3);
static const float MINOR = 0.35;
static const float MAJOR = 1.0;
// The |n·v| where anti-aliased lines have faded out, and where they start to: grazing angles,
// where the quad's edge is in perspective.
static const float GRAZING = 0.002;
static const float GRAZING_FADE = 0.02;

// `v`, a world point or offset, in the plane's coordinates, along its axes in name order.
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
    // The quad's center and the corner's offset, through the matrix apart: rounding far from the
    // origin then moves every corner alike, keeping the quad flat.
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

// Whether the pixel centered at `at`, `pixel` world units across, draws the line at 0: the one
// from three quarters of a pixel short of it to under a quarter past. A line on a pixel's center
// or edge, as the axes are with the camera over the origin, is a quarter pixel from tipping over.
bool on_axis(float at, float pixel) {
    float past = at + 0.75 * pixel;
    return past >= 0.0 && past < pixel;
}

// Whether that pixel draws a line at a multiple of `spacing` on either axis, as `on_axis` does.
bool on_lines(float2 at, float2 pixel, float spacing) {
    float2 past = frac((at + 0.75 * pixel) / spacing) * spacing;
    return past.x < pixel.x || past.y < pixel.y;
}

// For an orthographic view facing the plane squarely, where `pixel` is alike on both axes.
float4 crisp_lines(float2 at, float2 axes, float2 pixel, Grid grid) {
    // The finest power of ten at least FINEST pixels apart, and how far toward 10 * FINEST it is:
    // its fade.
    float finest = log10(max(pixel.x, pixel.y) * FINEST);
    float level = ceil(finest);
    float fade = level - finest;
    float spacing = pow(10.0, level);
    // A coarser line is a finer one too, and takes its larger weight.
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
    // Axes over lines, the second over the first; the second lies where the first coordinate is 0.
    if (on_axis(axes.x, pixel.x)) {
        return float4(grid.second, 1.0);
    }
    if (on_axis(axes.y, pixel.y)) {
        return float4(grid.first, 1.0);
    }
    return float4(GRAY, weight);
}

// The pixel's coverage by lines at multiples of `spacing`, `at` the coordinate across them and
// `pixel` its change per pixel: Ben Golus's pristine grid, "The Best Darn Grid Shader (Yet)"
// (2023), https://bgolus.medium.com/the-best-darn-grid-shader-yet-727f9278b9d8, for pixel-wide
// lines. A line is at most half the spacing wide, its edges ramped over a pixel and a half. Closer
// than two pixels, lines fade to their mean coverage, so they never alias into moiré. His case for
// lines wider than half the spacing is left out: pixel-wide lines over two pixels apart never are.
float coverage(float at, float pixel, float spacing) {
    // The spacings a pixel spans: a pixel-wide line's width in them.
    float cells = pixel / spacing;
    // Twice the distance to the nearest line, in those spaces.
    float across = 1.0 - abs(frac(at / spacing) * 2.0 - 1.0);
    float width = min(cells, 0.5);
    float drawn = 1.0 - smoothstep(width - cells * 1.5, width + cells * 1.5, across);
    return lerp(drawn, min(cells, 1.0), saturate(cells * 2.0 - 1.0));
}

// One coordinate's family of lines, `at` and `pixel` as in `coverage`: each power of ten weighed
// as in `crisp_lines`, anti-aliased, none coarser than `coarsest`, which coordinates are taken
// modulo.
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

// The axis at 0, drawn as `coverage` draws a line.
float axis(float at, float pixel) {
    return 1.0 - smoothstep(-0.5 * pixel, 2.5 * pixel, 2.0 * abs(at));
}

// For every view but `CRISP`'s.
float4 smooth_lines(float2 at, float2 axes, float2 pixel, Grid grid) {
    // No finer than the coordinates resolve where largest, about a ten millionth of the coarsest
    // spacing: past a float's precision.
    pixel = max(pixel, pow(10.0, grid.coarsest - 7.0));
    float gray = max(family(at.x, pixel.x, grid.coarsest), family(at.y, pixel.y, grid.coarsest));
    // Lines, then the first axis, then the second, as in `crisp_lines`; premultiplied until the
    // end.
    float4 color = float4(GRAY * gray, gray);
    color = lerp(color, float4(grid.first, 1.0), axis(axes.y, pixel.y));
    color = lerp(color, float4(grid.second, 1.0), axis(axes.x, pixel.x));
    // |n·v| at the fragment: the same everywhere orthographically; in perspective, the camera's
    // height above the plane over its distance.
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

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Grid grid = push.grids[input.instance];
    // Each family's footprint in plane units per pixel: the derivatives' length, as Ben Golus
    // takes it, not fwidth's sum, so lines keep their width whichever way they run.
    float2 dx = ddx(input.plane);
    float2 dy = ddy(input.plane);
    float2 pixel = float2(length(float2(dx.x, dy.x)), length(float2(dx.y, dy.y)));
    float4 color;
    if (grid.lines == CRISP) {
        color = crisp_lines(input.plane, input.axes, pixel, grid);
    } else {
        color = smooth_lines(input.plane, input.axes, pixel, grid);
    }
    // After the derivatives, which a discard before them would leave undefined.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = color;
    out.id = uint2(0, 0);
    return out;
}
