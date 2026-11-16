// SMAA 1x, enhanced subpixel morphological antialiasing (Jimenez, Echevarria, Sousa and Gutierrez,
// "SMAA: Enhanced Subpixel Morphological Antialiasing", 2012): antialiasing from the game's view's
// colors alone, without a history, at the reference's HIGH preset, whose functions these follow,
// by their names, under the MIT license: https://github.com/iryoku/smaa/blob/master/SMAA.hlsl
//
// In three passes, a pixel a thread. `edges_main` finds each pixel's edges with the pixels left of
// it and above it, where their lumas differ by the threshold or more, and by at least half the most
// any edge round them does, local contrast adaptation, so that a strong edge hides weak ones
// beside it. `weights_main` measures, for each pixel with an edge above or left of it, the line of
// edges it's on: as a diagonal of single steps first, and otherwise across or down, its distance to
// each end, by bilinear fetches that read two pixels' edges at once and the search texture, which
// says how far the last fetch's line goes, and how the edges crossing it there go; and from those,
// through the area texture, how much of the pixel and of its neighbor across the edge the line,
// revectorized as the edge it stands for, cuts off, which each takes of the other, rounding less
// near a sharp corner. `blend_main` blends each pixel with its neighbors across its strongest edges
// by those shares.
//
// The view's colors are of high dynamic range, where the reference's are as a display shows them:
// SMAA sees them as the grade and the tonemapper that follow it will show them, about: times the
// grade's slope, and rolled off toward 1 above a shoulder, invertibly, as Khronos PBR Neutral's
// shoulder rolls light off above 0.76, and, under no tonemapper, which clamps, above the last 256th
// below 1. Edges are found on the luma of the colors so shown and encoded by sRGB's curve, which the
// reference's threshold is for; and the blend mixes the colors so shown, linear as the reference's
// blend is, and takes them back, so that a pixel half covered by a bright sky shows half of it once
// the view is tonemapped, rather than as bright as the sky, and an edge below the shoulder blends
// exactly as it would after the grade's slope and the tonemapper.
//
// `Push` matches `SmaaPush` in src/smaa.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// The bindless set's sampler that filters linearly, clamped to the edges: `Sampler::Linear` with
// `Wrap::Clamp`.
static const uint LINEAR_CLAMP = 0;

// How many pixels across and down a workgroup takes: `GROUP` in src/smaa.rs.
static const uint GROUP = 8;

// The reference's HIGH preset: SMAA_THRESHOLD, SMAA_MAX_SEARCH_STEPS, SMAA_MAX_SEARCH_STEPS_DIAG and
// SMAA_CORNER_ROUNDING, as a fraction; and SMAA_LOCAL_CONTRAST_ADAPTATION_FACTOR.
static const float THRESHOLD = 0.1;
static const float MAX_SEARCH_STEPS = 16.0;
static const float MAX_SEARCH_STEPS_DIAG = 8.0;
static const float CORNER_ROUNDING = 0.25;
static const float LOCAL_CONTRAST_ADAPTATION = 2.0;

// The area texture, as src/smaa/lookup.rs makes it: the texels each orthogonal and diagonal
// pattern's square of distances takes each way, and a texel's size, of 160 by 80.
static const float AREA_MAX_DISTANCE = 16.0;
static const float AREA_MAX_DISTANCE_DIAG = 20.0;
static const float2 AREA_PIXEL_SIZE = float2(1.0 / 160.0, 1.0 / 80.0);
// The search texture's size before it was cropped, and as it's packed.
static const float2 SEARCH_SIZE = float2(66.0, 33.0);
static const float2 SEARCH_PACKED_SIZE = float2(64.0, 16.0);

struct Push {
    // The view's size in pixels.
    uint2 size;
    // What the passes read, textures of the bindless set's: the view's colors, its edges, its
    // blending weights and the lookup textures.
    uint colors;
    uint edges;
    uint weights;
    uint area;
    uint search;
    // What a pass writes, a storage image of the bindless set's.
    uint target;
    // How SMAA sees the view's light, as `seen` says: what it's multiplied by, each channel, the
    // grade's slope, which follows SMAA, and where the shoulder that then rolls it off toward 1
    // starts.
    float4 curve;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Light as SMAA sees it, as `seen` makes it: `shown`, from 0 to 1, and how far each channel of that
// is below 1, `below`, which keeps the precision `shown` loses as it nears 1, as it does for light
// up to a half float's largest.
struct Seen {
    float3 shown;
    float3 below;
};

// What the view's light is multiplied by, each channel, before the shoulder: the grade's slope,
// kept from 0, which takes nothing back.
float3 gain() {
    return max(push.curve.xyz, 1.0 / BRIGHTEST);
}

// `light` as SMAA sees it: times the gain, as it is where its brightest channel is no more than the
// shoulder, and otherwise scaled so that that channel rolls off toward 1, as Khronos PBR Neutral's
// shoulder rolls it, `1 - roll² / (peak - shoulder + roll)`, where `roll` is what the shoulder has
// left to 1, as `unseen` takes back. PBR Neutral's toe and desaturation, which change dark and
// bright colors a little, are left out.
Seen seen(float3 light) {
    float3 color = light * gain();
    float shoulder = push.curve.w;
    float roll = 1.0 - shoulder;
    float peak = max(color.r, max(color.g, color.b));
    Seen result;
    if (peak <= shoulder) {
        result.shown = color;
        result.below = 1.0 - color;
        return result;
    }
    // What the brightest channel is below 1 once rolled off; and each channel's, a share of the
    // way from what the color's ratio to that channel leaves to 1, without subtracting from 1.
    float left = roll * roll / (peak - shoulder + roll);
    result.shown = color * ((1.0 - left) / peak);
    result.below = (peak - color) / peak + color / peak * left;
    return result;
}

// The light SMAA sees as `shown`, `below` it 1, as a mix of what `seen` makes is.
float3 unseen(float3 shown, float3 below) {
    float shoulder = push.curve.w;
    float roll = 1.0 - shoulder;
    float least = min(below.r, min(below.g, below.b));
    float3 color = shown;
    if (least < roll) {
        // The brightest channel's light, from how far it's below 1, kept from 0, past which light
        // is past a half float's largest, which `finite` takes it to.
        float peak = roll * roll / max(least, 1.0e-30) + shoulder - roll;
        color = shown * (peak / (1.0 - least));
    }
    return color / gain();
}

// The luma edges are found on: Rec. 709's of `color` as `seen` shows it, encoded by sRGB's curve,
// as the reference finds it of colors as a display encodes them, which space what the eye tells
// apart.
float perceptual(float3 color) {
    float3 c = seen(finite(color)).shown;
    float3 encoded = select(c <= 0.0031308, c * 12.92, 1.055 * pow(c, float3(1.0 / 2.4)) - 0.055);
    return dot(encoded, float3(0.2126, 0.7152, 0.0722));
}

// The perceptual luma of the view's colors at `at`, clamped to the view, as the reference's point
// sampler clamps.
float luma_at(Texture2D colors, int2 at, int2 last) {
    return perceptual(colors.Load(int3(clamp(at, int2(0, 0), last), 0)).rgb);
}

[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void edges_main(uint3 pixel: SV_DispatchThreadID) {
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    Texture2D colors = textures[push.colors];
    int2 at = int2(pixel.xy);
    int2 last = int2(push.size) - 1;
    float l = luma_at(colors, at, last);
    float left = luma_at(colors, at + int2(-1, 0), last);
    float top = luma_at(colors, at + int2(0, -1), last);
    float4 delta;
    delta.xy = abs(l - float2(left, top));
    float2 edges = step(THRESHOLD, delta.xy);
    if (edges.x + edges.y > 0.0) {
        // The most any edge round the pixel differs by: its own right and bottom, and those left
        // of its left and above its top.
        float right = luma_at(colors, at + int2(1, 0), last);
        float bottom = luma_at(colors, at + int2(0, 1), last);
        delta.zw = abs(l - float2(right, bottom));
        float2 most = max(delta.xy, delta.zw);
        float left_left = luma_at(colors, at + int2(-2, 0), last);
        float top_top = luma_at(colors, at + int2(0, -2), last);
        delta.zw = abs(float2(left, top) - float2(left_left, top_top));
        most = max(most, delta.zw);
        float strongest = max(most.x, most.y);
        edges *= step(strongest, LOCAL_CONTRAST_ADAPTATION * delta.xy);
    }
    storage[push.target][uint3(pixel.xy, 0)] = float4(edges, 0.0, 0.0);
}

// The view's edges at `uv`, filtered: red where a pixel has an edge on its left, green on its top.
float2 edges_at(float2 uv) {
    return textures[push.edges].SampleLevel(samplers[LINEAR_CLAMP], uv, 0.0).rg;
}

float4 area_at(float2 uv) {
    return textures[push.area].SampleLevel(samplers[LINEAR_CLAMP], uv, 0.0);
}

// SMAASearchDiag1: how many pixels along `way` from `uv`, a pixel's middle, a diagonal of pixels
// with edges on their left and top goes on, and whether the last pixel looked at had both, `end`
// its edges, within the diagonal search's steps.
float2 search_diag1(float2 uv, float2 way, float4 metrics, out float2 end) {
    float4 coord = float4(uv, -1.0, 1.0);
    float3 t = float3(metrics.xy, 1.0);
    end = float2(0.0);
    while (coord.z < MAX_SEARCH_STEPS_DIAG - 1.0 && coord.w > 0.9) {
        coord.xyz = mad(t, float3(way, 1.0), coord.xyz);
        end = edges_at(coord.xy);
        coord.w = dot(end, float2(0.5, 0.5));
    }
    return coord.zw;
}

// SMAASearchDiag2, as `search_diag1`, for a diagonal of pixels with edges on their top and right,
// the right one the left edge of the pixel right of each; read a pixel at a time, as the
// reference's version that doesn't decode two from one bilinear fetch.
float2 search_diag2(float2 uv, float2 way, float4 metrics, out float2 end) {
    float4 coord = float4(uv, -1.0, 1.0);
    float3 t = float3(metrics.xy, 1.0);
    end = float2(0.0);
    while (coord.z < MAX_SEARCH_STEPS_DIAG - 1.0 && coord.w > 0.9) {
        coord.xyz = mad(t, float3(way, 1.0), coord.xyz);
        end.y = edges_at(coord.xy).g;
        end.x = edges_at(coord.xy + float2(metrics.x, 0.0)).r;
        coord.w = dot(end, float2(0.5, 0.5));
    }
    return coord.zw;
}

// SMAAAreaDiag: the areas of a diagonal line `dist` pixels from its ends, crossed at them as
// `crossing` says, in the area texture's right half.
float2 area_diag(float2 dist, float2 crossing) {
    float2 uv = mad(float2(AREA_MAX_DISTANCE_DIAG), crossing, dist);
    uv = AREA_PIXEL_SIZE * (uv + 0.5);
    uv.x += 0.5;
    return area_at(uv).rg;
}

// SMAACalculateDiagWeights: the blending weights of the pixel at `uv` with edges `e`, on its left
// and top, along a diagonal of single steps four pixels long or more, rising or falling, if it's on
// one; none otherwise. Each end's crossing edges are read a pixel at a time, as the reference's
// version that doesn't decode two from one bilinear fetch.
float2 calculate_diag_weights(float2 uv, float2 e, float4 metrics) {
    float2 weights = float2(0.0);
    float4 d;
    float2 end;
    // Rising to the right: pixels with edges on their left and top, down to the left and up to
    // the right, the lower end taking a last pixel with a top edge alone.
    if (e.r > 0.0) {
        d.xz = search_diag1(uv, float2(-1.0, 1.0), metrics, end);
        d.x += float(end.y > 0.9);
    } else {
        d.xz = float2(0.0);
    }
    d.yw = search_diag1(uv, float2(1.0, -1.0), metrics, end);
    if (d.x + d.y > 2.0) {
        float4 coords = mad(float4(-d.x, d.x, d.y, -d.y), metrics.xyxy, uv.xyxy);
        float4 c;
        c.x = edges_at(coords.xy + float2(-metrics.x, 0.0)).g;
        c.y = edges_at(coords.xy).r;
        c.z = edges_at(coords.zw + float2(metrics.x, 0.0)).g;
        c.w = edges_at(coords.zw + float2(metrics.x, -metrics.y)).r;
        // Each end's crossing edges as one value, none where the search ran out before it.
        float2 crossing = mad(float2(2.0), c.xz, c.yw);
        crossing = select(d.zw >= 0.9, float2(0.0), crossing);
        weights += area_diag(d.xy, crossing);
    }
    // Falling to the right: pixels with edges on their top and right, up to the left and down to
    // the right, the lower end taking a last pixel with a top edge alone; their areas are those of
    // the rising diagonal of the pixels right of them, flipped upside down, which the pixel is
    // above, so they swap.
    d.xz = search_diag2(uv, float2(-1.0, -1.0), metrics, end);
    if (edges_at(uv + float2(metrics.x, 0.0)).r > 0.0) {
        d.yw = search_diag2(uv, float2(1.0, 1.0), metrics, end);
        d.y += float(end.y > 0.9);
    } else {
        d.yw = float2(0.0);
    }
    if (d.x + d.y > 2.0) {
        float4 coords = mad(float4(-d.x, -d.x, d.y, d.y), metrics.xyxy, uv.xyxy);
        float4 c;
        c.x = edges_at(coords.xy + float2(-metrics.x, 0.0)).g;
        c.y = edges_at(coords.xy + float2(0.0, -metrics.y)).r;
        c.zw = edges_at(coords.zw + float2(metrics.x, 0.0)).gr;
        float2 crossing = mad(float2(2.0), c.xz, c.yw);
        crossing = select(d.zw >= 0.9, float2(0.0), crossing);
        weights += area_diag(d.xy, crossing).gr;
    }
    return weights;
}

// SMAASearchLength: how far past the last fetch of a search the line goes, from its edges `e`, as
// the search texture holds it, the texture's left half for searches to the left, `offset` 0, and
// its right half for those to the right, 0.5: 0, 1 or 2 pixels, times 127 over 255.
float search_length(float2 e, float offset) {
    float2 scale = SEARCH_SIZE * float2(0.5, -1.0);
    float2 bias = SEARCH_SIZE * float2(offset, 1.0);
    // To the texels' middles, then to texture coordinates of the packed texture.
    scale += float2(-1.0, 1.0);
    bias += float2(0.5, -0.5);
    scale *= 1.0 / SEARCH_PACKED_SIZE;
    bias *= 1.0 / SEARCH_PACKED_SIZE;
    return textures[push.search].SampleLevel(samplers[LINEAR_CLAMP], mad(scale, e, bias), 0.0).r;
}

// SMAASearchXLeft: where the line of top edges the pixel at `uv` is on ends to the left, in
// texture coordinates, a pixel's middle: by bilinear fetches two pixels apart, from a quarter of a
// pixel left of the pixel's middle and an eighth up, which tell four edges apart, two pixels' top
// and left edges, while both pixels have the line's edge and no edge crosses it, up to `end`.
float search_x_left(float2 uv, float end, float4 metrics) {
    float2 e = float2(0.0, 1.0);
    while (uv.x > end && e.g > 0.8281 && e.r == 0.0) {
        e = edges_at(uv);
        uv = mad(-float2(2.0, 0.0), metrics.xy, uv);
    }
    float offset = mad(-(255.0 / 127.0), search_length(e, 0.0), 3.25);
    return mad(metrics.x, offset, uv.x);
}

float search_x_right(float2 uv, float end, float4 metrics) {
    float2 e = float2(0.0, 1.0);
    while (uv.x < end && e.g > 0.8281 && e.r == 0.0) {
        e = edges_at(uv);
        uv = mad(float2(2.0, 0.0), metrics.xy, uv);
    }
    float offset = mad(-(255.0 / 127.0), search_length(e, 0.5), 3.25);
    return mad(-metrics.x, offset, uv.x);
}

float search_y_up(float2 uv, float end, float4 metrics) {
    float2 e = float2(1.0, 0.0);
    while (uv.y > end && e.r > 0.8281 && e.g == 0.0) {
        e = edges_at(uv);
        uv = mad(-float2(0.0, 2.0), metrics.xy, uv);
    }
    float offset = mad(-(255.0 / 127.0), search_length(e.gr, 0.0), 3.25);
    return mad(metrics.y, offset, uv.y);
}

float search_y_down(float2 uv, float end, float4 metrics) {
    float2 e = float2(1.0, 0.0);
    while (uv.y < end && e.r > 0.8281 && e.g == 0.0) {
        e = edges_at(uv);
        uv = mad(float2(0.0, 2.0), metrics.xy, uv);
    }
    float offset = mad(-(255.0 / 127.0), search_length(e.gr, 0.5), 3.25);
    return mad(-metrics.y, offset, uv.y);
}

// SMAAArea: the areas of an orthogonal line `dist` pixels from its ends, the area texture
// holding them by the distances' square roots, crossed at them as `e1` and `e2`, bilinear fetches
// of each end's crossing edges, a quarter toward the neighbor's side, say: rounded, so that
// filtering's precision doesn't move them between patterns.
float2 area_ortho(float2 dist, float e1, float e2) {
    float2 uv = mad(float2(AREA_MAX_DISTANCE), round(4.0 * float2(e1, e2)), dist);
    uv = AREA_PIXEL_SIZE * (uv + 0.5);
    return area_at(uv).rg;
}

// SMAADetectHorizontalCornerPattern: how much less a pixel on a line of top edges `d` pixels from
// its ends blends, where the nearer end's crossing edge goes on past the pixel's row, or its
// neighbor's, the corner of a shape, which rounds by the corner rounding alone. `coords` holds the
// line's left end, the pixel's row and its right end.
float2 horizontal_corners(float4 coords, float2 d, float4 metrics) {
    float2 left_right = step(d.xy, d.yx);
    float2 rounding = (1.0 - CORNER_ROUNDING) * left_right;
    // Less for pixels in the middle of a line, whose ends are as near.
    rounding /= left_right.x + left_right.y;
    float2 factor = float2(1.0);
    factor.x -= rounding.x * edges_at(coords.xy + float2(0.0, 1.0) * metrics.xy).r;
    factor.x -= rounding.y * edges_at(coords.zw + float2(1.0, 1.0) * metrics.xy).r;
    factor.y -= rounding.x * edges_at(coords.xy + float2(0.0, -2.0) * metrics.xy).r;
    factor.y -= rounding.y * edges_at(coords.zw + float2(1.0, -2.0) * metrics.xy).r;
    return saturate(factor);
}

// SMAADetectVerticalCornerPattern, as `horizontal_corners`, for a line of left edges, `coords`
// holding the pixel's column and the line's top end, and its column and bottom end.
float2 vertical_corners(float4 coords, float2 d, float4 metrics) {
    float2 left_right = step(d.xy, d.yx);
    float2 rounding = (1.0 - CORNER_ROUNDING) * left_right;
    rounding /= left_right.x + left_right.y;
    float2 factor = float2(1.0);
    factor.x -= rounding.x * edges_at(coords.xy + float2(1.0, 0.0) * metrics.xy).g;
    factor.x -= rounding.y * edges_at(coords.zw + float2(1.0, 1.0) * metrics.xy).g;
    factor.y -= rounding.x * edges_at(coords.xy + float2(-2.0, 0.0) * metrics.xy).g;
    factor.y -= rounding.y * edges_at(coords.zw + float2(-2.0, 1.0) * metrics.xy).g;
    return saturate(factor);
}

// SMAABlendingWeightCalculationPS: each pixel's blending weights, how much it takes of its
// neighbor above, red, and that neighbor of it, green, by its top edge, and of its neighbor to the
// left, blue, and that neighbor of it, alpha, by its left edge; none where it has neither.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void weights_main(uint3 pixel: SV_DispatchThreadID) {
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    // A pixel's size in texture coordinates, and the view's size in pixels.
    float4 metrics = float4(1.0 / float2(push.size), float2(push.size));
    float2 uv = (float2(pixel.xy) + 0.5) * metrics.xy;
    float2 pixcoord = float2(pixel.xy) + 0.5;
    // Where the searches start, between pixels, and where they stop.
    float4 offset0 = mad(metrics.xyxy, float4(-0.25, -0.125, 1.25, -0.125), uv.xyxy);
    float4 offset1 = mad(metrics.xyxy, float4(-0.125, -0.25, -0.125, 1.25), uv.xyxy);
    float4 offset2 = mad(metrics.xxyy, float4(-2.0, 2.0, -2.0, 2.0) * MAX_SEARCH_STEPS,
                         float4(offset0.xz, offset1.yw));
    float4 weights = float4(0.0);
    float2 e = textures[push.edges].Load(int3(pixel.xy, 0)).rg;
    if (e.g > 0.0) {
        // A diagonal has both edges, so its search from the top edge alone finds it; and a pixel
        // on one takes its weights alone.
        weights.rg = calculate_diag_weights(uv, e, metrics);
        if (weights.r == -weights.g) {
            float2 d;
            float3 coords;
            coords.x = search_x_left(offset0.xy, offset2.x, metrics);
            // A quarter of a pixel up, so that each end's two crossing edges, below the line and
            // above it, read apart from one bilinear fetch.
            coords.y = offset1.y;
            d.x = coords.x;
            float e1 = edges_at(coords.xy).r;
            coords.z = search_x_right(offset0.zw, offset2.y, metrics);
            d.y = coords.z;
            d = abs(round(mad(metrics.zz, d, -pixcoord.xx)));
            float e2 = edges_at(coords.zy + float2(metrics.x, 0.0)).r;
            float2 areas = area_ortho(sqrt(d), e1, e2);
            coords.y = uv.y;
            weights.rg = areas * horizontal_corners(coords.xyzy, d, metrics);
        } else {
            // On a diagonal, its left edge is the diagonal's too.
            e.r = 0.0;
        }
    }
    if (e.r > 0.0) {
        float2 d;
        float3 coords;
        coords.y = search_y_up(offset1.xy, offset2.z, metrics);
        coords.x = offset0.x;
        d.x = coords.y;
        float e1 = edges_at(coords.xy).g;
        coords.z = search_y_down(offset1.zw, offset2.w, metrics);
        d.y = coords.z;
        d = abs(round(mad(metrics.ww, d, -pixcoord.yy)));
        float e2 = edges_at(coords.xz + float2(0.0, metrics.y)).g;
        float2 areas = area_ortho(sqrt(d), e1, e2);
        coords.x = uv.x;
        weights.ba = areas * vertical_corners(coords.xyxz, d, metrics);
    }
    storage[push.target][uint3(pixel.xy, 0)] = weights;
}

// The blending weights at `at`, none outside the view, which has no edges past its sides.
float4 weights_at(int2 at) {
    bool inside = all(at >= int2(0, 0)) && all(at < int2(push.size));
    return inside ? textures[push.weights].Load(int3(at, 0)) : float4(0.0);
}

// SMAANeighborhoodBlendingPS: each pixel blended with its neighbors across its stronger edges,
// across or down, each by its share, the pixel's own weights for those above and to the left and
// theirs for those below and to the right.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void blend_main(uint3 pixel: SV_DispatchThreadID) {
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    int2 at = int2(pixel.xy);
    Texture2D colors = textures[push.colors];
    float4 drawn = colors.Load(int3(at, 0));
    float3 color = finite(drawn.rgb);
    // How much the pixel takes of its neighbor to the right, below, to the left and above.
    float4 own = weights_at(at);
    float4 a = float4(weights_at(at + int2(1, 0)).a, weights_at(at + int2(0, 1)).g, own.b, own.r);
    if (dot(a, float4(1.0)) >= 1.0e-5) {
        bool across = max(a.x, a.z) > max(a.y, a.w);
        float2 shares = across ? a.xz : a.yw;
        int2 way = across ? int2(1, 0) : int2(0, 1);
        int2 last = int2(push.size) - 1;
        float3 after = finite(colors.Load(int3(clamp(at + way, int2(0, 0), last), 0)).rgb);
        float3 before = finite(colors.Load(int3(clamp(at - way, int2(0, 0), last), 0)).rgb);
        Seen own = seen(color);
        Seen next = seen(after);
        Seen previous = seen(before);
        // Each neighbor lerped toward by its share, the two weighted by their shares, as the
        // reference's two bilinear fetches mix them; what's below 1 alike.
        float total = shares.x + shares.y;
        float3 shown = (lerp(own.shown, next.shown, shares.x) * shares.x
                        + lerp(own.shown, previous.shown, shares.y) * shares.y)
            / total;
        float3 below = (lerp(own.below, next.below, shares.x) * shares.x
                        + lerp(own.below, previous.below, shares.y) * shares.y)
            / total;
        color = finite(unseen(shown, below));
    }
    storage[push.target][uint3(pixel.xy, 0)] = float4(color, drawn.a);
}
