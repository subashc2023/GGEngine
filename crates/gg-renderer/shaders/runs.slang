// A run of a frame's culling, which culling.slang and visibility.slang share: `GpuRun` in
// src/mesh/cull.rs, field for field.

// A run of a mesh's instances, drawn with one shader.
struct Run {
    // Its first instance among the frame's, and how many it has.
    uint first;
    uint count;
    // The first word of its instances' parameters among the frame's, and how many each takes.
    uint params;
    uint params_words;
    // The sphere round its mesh's vertices, in the mesh's own space: its center and radius, which
    // is infinite for a mesh whose vertices the renderer can't read, which is never culled.
    float4 sphere;
    // Its mesh's levels' first in the frame's table, and how many there are.
    uint levels;
    uint level_count;
    // Its first instance in each view's arrays, which hold room for all of its instances, and the
    // first word of its parameters there.
    uint output;
    uint params_output;
    // Its first command in each view's, one for each level.
    uint commands;
    // Its first block among the frame's, and how many it has.
    uint blocks;
    uint block_count;
    // For a run drawn by its mesh's meshlets: its mesh's first in the table of meshlets' bounds,
    // or `NO_MESHLETS` for a run drawn by its levels' indices; where its draw is in each view's
    // meshlets, in words, and how many meshlets its draw has room for; and whether a meshlet's
    // cone may cull it, as it does where the run draws its triangles' fronts alone.
    uint meshlets;
    uint meshlet_draw;
    uint meshlet_room;
    uint cones;
    // The address of its mesh's buffer, its low and high words, for the visibility buffer's
    // shading to read its meshlets' vertices through: words, as the culling's shaders hold no
    // address in a buffer, as cull.slang says why.
    uint2 vertices;
};
