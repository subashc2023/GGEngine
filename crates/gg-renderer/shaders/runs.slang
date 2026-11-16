// A run of a frame's culling, shared by culling.slang and visibility.slang: `GpuRun` in
// src/mesh/cull.rs, field for field.

// A run of a mesh's instances, drawn with one shader.
struct Run {
    // Among the frame's instances.
    uint first;
    uint count;
    // Its instances' parameters: their first word among the frame's, and the words each takes.
    uint params;
    uint params_words;
    // Round its mesh's vertices, in mesh space: center and radius, infinite (never culled) where
    // the renderer can't read them.
    float4 sphere;
    // Its mesh's levels in the frame's table.
    uint levels;
    uint level_count;
    // Its first instance in each view's arrays, which have room for all, and its parameters' first
    // word there.
    uint output;
    uint params_output;
    // Its first command in each view's, one per level.
    uint commands;
    // Among the frame's blocks.
    uint blocks;
    uint block_count;
    // For a run drawn by its mesh's meshlets:
    // - `meshlets`: its mesh's first in the meshlets' bounds table; `NO_MESHLETS` for a run drawn
    //   by its levels' indices.
    // - `meshlet_draw`: its draw in each view's meshlets, in words; `meshlet_room`, the meshlets
    //   it has room for.
    // - `cones`: whether a meshlet's cone may cull it, as where it draws front faces alone.
    uint meshlets;
    uint meshlet_draw;
    uint meshlet_room;
    uint cones;
    // Its mesh buffer's address, low word first, for the visibility buffer's shading to read
    // meshlets' vertices: words, as cull.slang says why.
    uint2 vertices;
};
