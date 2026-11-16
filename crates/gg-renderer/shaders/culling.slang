// Shared by cull.slang and meshlets.slang: runs, levels and views as runs.slang and `GpuLevel` and
// `GpuView` in src/mesh/cull.rs have them, field for field, and the sphere tests. Each includer
// declares `push`, with `instances`, `block_count`, `counts` and `pyramid`.

#include "runs.slang"

// A run's instances per workgroup: `BLOCK` in src/mesh/cull.rs.
static const uint BLOCK = 64;
// The most workgroups a dispatch takes across: `MOST_GROUPS` in src/mesh/cull.rs.
static const uint MOST_GROUPS = 65535;
// A mesh's most levels of detail: `MAX_LEVELS` in src/mesh.rs.
static const uint MAX_LEVELS = 8;
// An instance a view doesn't draw.
static const uint CULLED = 0xFFFFFFFF;
// `Run::meshlets` without meshlets: `NO_MESHLETS` in src/mesh/cull.rs.
static const uint NO_MESHLETS = 0xFFFFFFFF;
// The pixels a level's error may span on screen: `LOD_ERROR_PIXELS` in src/mesh.rs.
static const float LOD_ERROR_PIXELS = 1.0;
// The game's view's first pass's slot among the views: `FIRST` in src/mesh/cull.rs.
static const uint FIRST = 0;

// What a view keeps of what's inside its planes: `Keep` in src/mesh/cull.rs.
// - `KEEP_ALL`: all, for a cascade.
// - `KEEP_SHOWN`: what last frame found unhidden, for the game's view's first pass.
// - `KEEP_UNHIDDEN`: what the first pass didn't keep and the pyramid doesn't hide, for its second.
static const uint KEEP_ALL = 0;
static const uint KEEP_SHOWN = 1;
static const uint KEEP_UNHIDDEN = 2;

// A level of detail: its indices, its surface's `error` from the mesh's, and its meshlets among
// the mesh's.
struct Level {
    uint start;
    uint count;
    float error;
    uint meshlet_first;
    uint meshlet_count;
};

// A `VkDrawIndexedIndirectCommand`.
struct Command {
    uint index_count;
    uint instance_count;
    uint first_index;
    int vertex_offset;
    uint first_instance;
};

struct View {
    // `p` is inside a plane where `dot(xyz, p) + w` is at least 0; a sphere reaches inside where
    // that's at least `-radius * length(xyz)`.
    float4 planes[6];
    // As `LodView`: `dot(w, p)` is clip W; `w_slope`, its change per world unit; `pixels_per_unit`,
    // pixels per world unit at W 1.
    float4 w;
    // Likewise clip X, Y and Z, for the pyramid's test.
    float4 x;
    float4 y;
    float4 z;
    float w_slope;
    float pixels_per_unit;
    // `slot` places its parts of the arrays, scratch and commands; `keep` is a `KEEP_` constant.
    uint slot;
    uint keep;
    // For `KEEP_UNHIDDEN`: its size in pixels and pyramid levels.
    uint width;
    uint height;
    uint pyramid_levels;
    // Its meshlets' draws among the culling's commands, in words.
    uint meshlets;
    // Where it sees from, and 1, under perspective; or the way it looks, and 0, without.
    float4 eye;
    // For the game's view's passes: the index, past every view's, of its kept instances'
    // last-frame transforms in the kept transforms. `NO_PREVIOUS` for a cascade.
    uint previous;
    uint unused[3];
};

// `View::previous` of a view keeping no last-frame transforms.
static const uint NO_PREVIOUS = 0xFFFFFFFF;

// View `slot`'s codes in the scratch, at each run's `output`: `level | place << 8`, the place
// among its level's in its block, or `CULLED`.
uint codes_at(uint slot) {
    return push.instances + slot * (push.instances + push.block_count * MAX_LEVELS);
}

// View `slot`'s blocks' levels in the scratch, `MAX_LEVELS` per block: the view's count per level
// (`choose_main`), then the first's place in the run's room (`offsets_main`).
uint block_levels_at(uint slot) {
    return codes_at(slot) + push.instances;
}

// Workgroups dispatched across, each taking blocks that many apart.
uint groups() {
    return min(push.block_count, MOST_GROUPS);
}

// The most a transform's columns stretch any way: the square root of their dot products' largest
// eigenvalue, which Gershgorin's circles bound by the largest row sum of magnitudes. Exact for
// columns at right angles (rotation and scale), above for a shear.
float stretch(float3 x, float3 y, float3 z) {
    float xy = abs(dot(x, y));
    float xz = abs(dot(x, z));
    float yz = abs(dot(y, z));
    return sqrt(max(dot(x, x) + xy + xz, max(dot(y, y) + xy + yz, dot(z, z) + xz + yz)));
}

// Whether the world sphere reaches inside every plane of `view`.
bool reaches(View view, float3 center, float radius) {
    bool inside = true;
    for (uint i = 0; i < 6; i++) {
        float4 plane = view.planes[i];
        if (dot(plane.xyz, center) + plane.w < -radius * length(plane.xyz)) {
            inside = false;
        }
    }
    return inside;
}

// The least and most clip coordinate over W across a sphere of `radius`, its center at `c` and W
// `w`, the row's XYZ `length` long. Without perspective W is 1. With it, W is the distance ahead,
// and the sphere a disc in the plane of ahead and the row's axis, bound by its tangents through
// the camera (Mara and McGuire, "2D Polyhedral Bounds of a Clipped, Perspective-Projected 3D
// Sphere", 2013). For a sphere wholly ahead of the camera.
float2 across(float c, float length, float w, float radius, float w_slope) {
    if (w_slope == 0.0) {
        return float2(c - radius * length, c + radius * length);
    }
    float a = c / length;
    float tangent = sqrt(max(a * a + w * w - radius * radius, 0.0));
    float least = (a * tangent - radius * w) / (w * tangent + radius * a);
    float most = (a * tangent + radius * w) / (w * tangent - radius * a);
    return float2(least, most) * length;
}

// Whether `view`'s depth pyramid hides the world sphere: every pixel it may cover drew nearer than
// its nearest point. Its screen rectangle, a pixel wider each way against rounding, falls within
// 2 by 2 texels of the level whose texels are as wide; their farthest bounds what's drawn there.
bool hides(View view, float3 center, float radius) {
    float4 p = float4(center, 1.0);
    float w = dot(view.w, p);
    // The W of its nearest point: one that reaches the camera's plane has no rectangle.
    float near = w - radius * view.w_slope;
    if (!(near > 0.0)) {
        return false;
    }
    float3 lengths = float3(length(view.x.xyz), length(view.y.xyz), length(view.z.xyz));
    float2 xs = across(dot(view.x, p), lengths.x, w, radius, view.w_slope);
    float2 ys = across(dot(view.y, p), lengths.y, w, radius, view.w_slope);
    // As near as it comes: no Z over the sphere is more, nor W less.
    float depth = (dot(view.z, p) + radius * lengths.z) / near;
    float2 size = float2(view.width, view.height);
    float2 least = (float2(xs.x, ys.x) * 0.5 + 0.5) * size - 1.0;
    float2 most = (float2(xs.y, ys.y) * 0.5 + 0.5) * size + 1.0;
    if (any(isnan(least)) || any(isnan(most))) {
        return false;
    }
    uint2 first = uint2(clamp(least, float2(0.0), size - 1.0));
    uint2 last = uint2(clamp(most, float2(0.0), size - 1.0));
    // The least level whose texels, 2^(level + 1) pixels a side, are as wide as the rectangle,
    // which then spans at most 2 each way.
    uint span = max(last.x - first.x, last.y - first.y) + 1;
    uint level = span <= 1 ? 0 : firstbithigh(span - 1);
    level = min(level, view.pyramid_levels - 1);
    uint at = 0;
    uint2 texels = (uint2(view.width, view.height) + 1) / 2;
    for (uint i = 0; i < level; i++) {
        at += texels.x * texels.y;
        texels = (texels + 1) / 2;
    }
    uint2 a = first >> (level + 1);
    uint2 b = last >> (level + 1);
    float farthest = min(
        min(push.pyramid[at + a.y * texels.x + a.x], push.pyramid[at + a.y * texels.x + b.x]),
        min(push.pyramid[at + b.y * texels.x + a.x], push.pyramid[at + b.y * texels.x + b.x]));
    return depth < farthest;
}

// Adds `value` to the 64-bit count at `low` in `push.counts`, low word first.
void add_wide(uint low, uint value) {
    uint was;
    InterlockedAdd(push.counts[low], value, was);
    if (was + value < was) {
        InterlockedAdd(push.counts[low + 1], 1);
    }
}
