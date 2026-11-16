// SMAA 1x (Jimenez, Echevarria, Sousa and Gutierrez, "SMAA: Enhanced Subpixel Morphological
// Antialiasing", 2012) at the reference's HIGH preset, from the view's colors alone, a pixel a
// thread. Functions follow the reference's, by name, under the MIT license:
// https://github.com/iryoku/smaa/blob/master/SMAA.hlsl
//
// - `edges_main`: left and top edges by luma, with local contrast adaptation.
// - `weights_main`: what each pixel and its neighbor across an edge take of each other, by the line
//   of edges they're on, through the search and area textures.
// - `blend_main`: each pixel blended across its strongest edges.
//
// The view's colors are HDR; the reference's are as a display shows them. So SMAA sees light as
// the grade and tonemapper will show it, about: times the grade's slope, then rolled off
// invertibly toward 1 above a shoulder (Khronos PBR Neutral's 0.76; under no tonemapper, which
// clamps, the last 256th below 1). Edges are found on its sRGB-encoded luma, which the reference's
// threshold is for; the blend mixes it and takes it back. So a pixel half covered by a bright sky
// shows half of it once tonemapped, and an edge below the shoulder blends exactly as after the
// grade and tonemapper.
//
// `Push` matches `SmaaPush` in src/smaa.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// The bindless set's linear, edge-clamped sampler: `Sampler::Linear` with `Wrap::Clamp`.
static const uint LINEAR_CLAMP = 0;

// Pixels across and down a workgroup: `GROUP` in src/smaa.rs.
static const uint GROUP = 8;

// The reference's HIGH preset: SMAA_THRESHOLD, SMAA_MAX_SEARCH_STEPS, SMAA_MAX_SEARCH_STEPS_DIAG,
// SMAA_CORNER_ROUNDING as a fraction, and SMAA_LOCAL_CONTRAST_ADAPTATION_FACTOR.
static const float THRESHOLD = 0.1;
static const float MAX_SEARCH_STEPS = 16.0;
static const float MAX_SEARCH_STEPS_DIAG = 8.0;
static const float CORNER_ROUNDING = 0.25;
static const float LOCAL_CONTRAST_ADAPTATION = 2.0;

// The area texture, 160 by 80, as src/smaa/lookup.rs makes it: the texels each orthogonal and
// diagonal pattern's square of distances takes each way, and a texel's size.
static const float AREA_MAX_DISTANCE = 16.0;
static const float AREA_MAX_DISTANCE_DIAG = 20.0;
static const float2 AREA_PIXEL_SIZE = float2(1.0 / 160.0, 1.0 / 80.0);
// The search texture's size before it was cropped, and as it's packed.
static const float2 SEARCH_SIZE = float2(66.0, 33.0);
static const float2 SEARCH_PACKED_SIZE = float2(64.0, 16.0);

struct Push {
    // In pixels.
    uint2 size;
    // Bindless textures: the view's colors, edges, weights and the lookup textures.
    uint colors;
    uint edges;
    uint weights;
    uint area;
    uint search;
    // What a pass writes: a bindless storage image.
    uint target;
    // How `seen` sees light: the grade's slope per channel, then where the shoulder starts.
    float4 curve;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Light as `seen` shows it, from 0 to 1, and each channel's distance below 1, which keeps the
// precision `shown` loses near 1, up to a half float's largest light.
struct Seen {
    float3 shown;
    float3 below;
};

// The grade's slope per channel, kept from 0, which takes nothing back.
float3 gain() {
    return max(push.curve.xyz, 1.0 / BRIGHTEST);
}

// `light` as SMAA sees it: times the gain, then, past the shoulder, scaled so its brightest channel
// rolls off toward 1 as PBR Neutral's shoulder does: `1 - roll² / (peak - shoulder + roll)`, `roll`
// being 1 less the shoulder. `unseen` takes it back. PBR Neutral's toe and desaturation, which
// change dark and bright colors a little, are left out.
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
    // The brightest channel's distance below 1 once rolled off; each channel's, a share of the way
    // from what its ratio to that channel leaves to 1, without subtracting from 1.
    float left = roll * roll / (peak - shoulder + roll);
    result.shown = color * ((1.0 - left) / peak);
    result.below = (peak - color) / peak + color / peak * left;
    return result;
}

// The light `seen` shows as `shown` and `below`, or as a mix of its results.
float3 unseen(float3 shown, float3 below) {
    float shoulder = push.curve.w;
    float roll = 1.0 - shoulder;
    float least = min(below.r, min(below.g, below.b));
    float3 color = shown;
    if (least < roll) {
        // The brightest channel's light, from its distance below 1, kept from 0: past it, light
        // passes a half float's largest, which `finite` takes it to.
        float peak = roll * roll / max(least, 1.0e-30) + shoulder - roll;
        color = shown * (peak / (1.0 - least));
    }
    return color / gain();
}

// The Rec. 709 luma of `color` as `seen` shows it, sRGB-encoded as the reference's colors are.
float perceptual(float3 color) {
    float3 c = seen(finite(color)).shown;
    float3 encoded = select(c <= 0.0031308, c * 12.92, 1.055 * pow(c, float3(1.0 / 2.4)) - 0.055);
    return dot(encoded, float3(0.2126, 0.7152, 0.0722));
}

// The perceptual luma at `at`, clamped to the view as the reference's point sampler clamps.
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
        // The most any edge round the pixel differs by.
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

// SMAASearchDiag1: the steps along `way` a diagonal of left and top edges goes from `uv`, a
// pixel's middle, and whether its last pixel had both; `end` its edges.
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

// SMAASearchDiag2, for top and right edges, read a pixel at a time: the reference's version that
// doesn't decode two from one bilinear fetch.
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

// SMAAAreaDiag, from the area texture's right half.
float2 area_diag(float2 dist, float2 crossing) {
    float2 uv = mad(float2(AREA_MAX_DISTANCE_DIAG), crossing, dist);
    uv = AREA_PIXEL_SIZE * (uv + 0.5);
    uv.x += 0.5;
    return area_at(uv).rg;
}

// SMAACalculateDiagWeights: the weights on a diagonal four pixels long or more, none off one.
// Crossing edges are read a pixel at a time, as in `search_diag2`.
float2 calculate_diag_weights(float2 uv, float2 e, float4 metrics) {
    float2 weights = float2(0.0);
    float4 d;
    float2 end;
    // Rising to the right: left and top edges; the lower end takes a last top edge alone.
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
    // Falling to the right: top and right edges, likewise. Its areas are the flipped rising
    // diagonal's right of it, which the pixel is above, so they swap.
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

// SMAASearchLength: how far past a search's last fetch the line goes, 0, 1 or 2 pixels times
// 127 / 255. `offset` 0 for searches left, the texture's left half; 0.5 for right.
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

// SMAASearchXLeft: where the line of top edges at `uv` ends to the left, a pixel's middle, in
// texture coordinates. Each bilinear fetch tells two pixels' top and left edges apart.
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

// SMAAArea: `dist` is the square roots of the distances to the line's ends. The crossings `e1` and
// `e2` are rounded, so filtering's precision doesn't move them between patterns.
float2 area_ortho(float2 dist, float e1, float e2) {
    float2 uv = mad(float2(AREA_MAX_DISTANCE), round(4.0 * float2(e1, e2)), dist);
    uv = AREA_PIXEL_SIZE * (uv + 0.5);
    return area_at(uv).rg;
}

// SMAADetectHorizontalCornerPattern: less blending at a shape's corner, where the nearer end's
// crossing edge goes on past the row. `coords`: the line's left end, the pixel's row, its right
// end.
float2 horizontal_corners(float4 coords, float2 d, float4 metrics) {
    float2 left_right = step(d.xy, d.yx);
    float2 rounding = (1.0 - CORNER_ROUNDING) * left_right;
    // Halved where both ends are as near.
    rounding /= left_right.x + left_right.y;
    float2 factor = float2(1.0);
    factor.x -= rounding.x * edges_at(coords.xy + float2(0.0, 1.0) * metrics.xy).r;
    factor.x -= rounding.y * edges_at(coords.zw + float2(1.0, 1.0) * metrics.xy).r;
    factor.y -= rounding.x * edges_at(coords.xy + float2(0.0, -2.0) * metrics.xy).r;
    factor.y -= rounding.y * edges_at(coords.zw + float2(1.0, -2.0) * metrics.xy).r;
    return saturate(factor);
}

// SMAADetectVerticalCornerPattern. `coords`: the pixel's column with the line's top end, then with
// its bottom end.
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

// SMAABlendingWeightCalculationPS: by its top edge, what the pixel takes of its upper neighbor,
// red, and that of it, green; by its left edge, of its left neighbor, blue, and that of it, alpha.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void weights_main(uint3 pixel: SV_DispatchThreadID) {
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    // A pixel's size in texture coordinates, then the view's in pixels.
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
        // A diagonal has both edges; its pixels take its weights alone.
        weights.rg = calculate_diag_weights(uv, e, metrics);
        if (weights.r == -weights.g) {
            float2 d;
            float3 coords;
            coords.x = search_x_left(offset0.xy, offset2.x, metrics);
            // A quarter pixel up, so one bilinear fetch tells each end's two crossing edges apart.
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

// The weights at `at`; none outside the view, which has no edges past its sides.
float4 weights_at(int2 at) {
    bool inside = all(at >= int2(0, 0)) && all(at < int2(push.size));
    return inside ? textures[push.weights].Load(int3(at, 0)) : float4(0.0);
}

// SMAANeighborhoodBlendingPS: each pixel blended across its stronger edges, across or down, by its
// own weights above and left and theirs below and right.
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
    // Its shares of its right, lower, left and upper neighbors.
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
        // A lerp toward each neighbor by its share, the two weighted by their shares, as the
        // reference's two bilinear fetches mix them; `below` alike.
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
