// What cull.slang and meshlets.slang share: the runs, levels and views a frame culls for, as
// runs.slang, and `GpuLevel` and `GpuView` in src/mesh/cull.rs, have them field for field, and the
// tests a sphere is culled by. Each module that includes it declares its own push constants,
// `push`, with the fields these read: `instances`, `block_count`, `counts` and `pyramid`.

#include "runs.slang"

// How many of a run's instances a workgroup takes: `BLOCK` in src/mesh/cull.rs.
static const uint BLOCK = 64;
// The most workgroups a dispatch takes across: `MOST_GROUPS` in src/mesh/cull.rs.
static const uint MOST_GROUPS = 65535;
// The most levels of detail a mesh has: `MAX_LEVELS` in src/mesh.rs.
static const uint MAX_LEVELS = 8;
// Where an instance a view doesn't draw goes: nowhere.
static const uint CULLED = 0xFFFFFFFF;
// What a run without meshlets has for its first meshlet: `NO_MESHLETS` in src/mesh/cull.rs.
static const uint NO_MESHLETS = 0xFFFFFFFF;
// How many pixels a level's error may span on screen: `LOD_ERROR_PIXELS` in src/mesh.rs.
static const float LOD_ERROR_PIXELS = 1.0;
// The slot of the game's view's first pass among the views: `FIRST` in src/mesh/cull.rs.
static const uint FIRST = 0;

// What a view keeps of what reaches inside its planes, as `Keep` in src/mesh/cull.rs has it: all of
// it, as a cascade does; what the last frame found unhidden, as the game's view's first pass does;
// or what its first pass didn't keep and the depth pyramid doesn't hide, as its second does.
static const uint KEEP_ALL = 0;
static const uint KEEP_SHOWN = 1;
static const uint KEEP_UNHIDDEN = 2;

// A level of detail: its indices, how far its surface strays from the mesh's own, and its
// meshlets among its mesh's, the first and how many, for a run drawn by them.
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

// A view that draws the runs.
struct View {
    // A point `p` is inside a plane where `dot(xyz, p) + w` is at least 0, and a sphere reaches
    // inside it where that's at least minus its radius times the length of `xyz`.
    float4 planes[6];
    // As `LodView`: what a point's clip W is the dot product of, how much W changes with a world
    // unit's distance, and the pixels a world unit spans where W is 1.
    float4 w;
    // What a point's clip X, Y and Z are the dot products of, which the pyramid's test projects
    // spheres by.
    float4 x;
    float4 y;
    float4 z;
    float w_slope;
    float pixels_per_unit;
    // Its place among the views, which its parts of the arrays, the scratch and the commands are
    // at, and what it keeps, one of `KEEP_ALL`, `KEEP_SHOWN` and `KEEP_UNHIDDEN`.
    uint slot;
    uint keep;
    // For `KEEP_UNHIDDEN`, its size in pixels, and how many levels its pyramid has.
    uint width;
    uint height;
    uint pyramid_levels;
    // Where its meshlets' draws start among the culling's commands, in words.
    uint meshlets;
    // Where it sees from, and 1, under perspective; or the way it looks, and 0, without.
    float4 eye;
    // For the game's view's passes, where the transforms the frame before of the instances it
    // keeps go among the kept transforms' arrays, past every view's, by index; `NO_PREVIOUS` for a
    // cascade's, which keeps none.
    uint previous;
    uint unused[3];
};

// What a view that keeps no transforms of the frame before has for `View::previous`.
static const uint NO_PREVIOUS = 0xFFFFFFFF;

// Where the codes of the view in slot `slot` start in the scratch: each instance's level, and its
// place among those of its level in its block, as `level | place << 8`, or `CULLED`, at its run's
// `output`.
uint codes_at(uint slot) {
    return push.instances + slot * (push.instances + push.block_count * MAX_LEVELS);
}

// Where the blocks' levels of the view in slot `slot` start in the scratch: for each block, by its
// index times `MAX_LEVELS`, how many of each level the view keeps of it, as `choose_main` counts
// them, and then where the first of them goes in its run's room, as `offsets_main` places them.
uint block_levels_at(uint slot) {
    return codes_at(slot) + push.instances;
}

// How many workgroups were dispatched across, each taking every that many blocks in turn.
uint groups() {
    return min(push.block_count, MOST_GROUPS);
}

// The most `columns`, a transform's linear part's columns, stretch any way: the square root of the
// largest eigenvalue of their dot products, which Gershgorin's circles bound by the largest sum of
// a row's magnitudes, exactly where the columns are at right angles, as a rotation and scale's
// are, and above it for a shear.
float stretch(float3 x, float3 y, float3 z) {
    float xy = abs(dot(x, y));
    float xz = abs(dot(x, z));
    float yz = abs(dot(y, z));
    return sqrt(max(dot(x, x) + xy + xz, max(dot(y, y) + xy + yz, dot(z, z) + xz + yz)));
}

// Whether the sphere round `center` of `radius`, in the world, reaches inside every plane of
// `view`.
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

// The least and the most of a clip coordinate over W across a sphere of `radius` round a center
// whose coordinate is `c`, of a row whose XYZ is `length` long, and whose W is `w`, `w_slope` as
// the view has it. Without perspective, W is 1. With it, W is the distance ahead of the camera, and
// the row a scale times the axis it's along, at right angles to the way ahead: the sphere is a disc
// in the plane of the two, `c / length` along the axis and `w` ahead, whose tangents through the
// camera bound it (Mara and McGuire, "2D Polyhedral Bounds of a Clipped, Perspective-Projected 3D
// Sphere", 2013), for a sphere wholly ahead of the camera.
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

// Whether `view`'s depth pyramid hides the sphere round `center` of `radius`, in the world: whether
// what's drawn in every pixel the sphere may cover is nearer than its nearest point. Its rectangle
// on screen, a pixel wider each way against rounding, falls within 2 by 2 texels of the level whose
// texels are at least as wide; the farthest of those is as far as anything there is drawn.
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
    // The least level whose texels, 2 to the level plus 1 pixels a side, are as wide as the
    // rectangle, which then reaches 2 at most each way.
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

// Adds `value` to the 64-bit count whose low word is at `low` in `push.counts` and high word after
// it.
void add_wide(uint low, uint value) {
    uint was;
    InterlockedAdd(push.counts[low], value, was);
    if (was + value < was) {
        InterlockedAdd(push.counts[low + 1], 1);
    }
}
