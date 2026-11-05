// Writes indirect draws' commands, their instances' colors and two counts in a compute pass, and
// draws each instance as a triangle over a pixel of its own, so that a test can read back what the
// GPU makes of the graph's own buffers, indirect draws that start from an instance their commands
// name, and a buffer read back.

// A `VkDrawIndexedIndirectCommand`.
struct Command {
    uint index_count;
    uint instance_count;
    uint first_index;
    int vertex_offset;
    uint first_instance;
};

struct Push {
    // Writing alone: where the commands go, and the counts read back.
    Command* commands;
    uint* counts;
    // Each instance's color.
    float4* colors;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("compute")]
[numthreads(1, 1, 1)]
void write_main() {
    // Two instances of a triangle, from the second, then none.
    Command drawn = { 3, 2, 0, 0, 1 };
    Command none = { 3, 0, 0, 0, 0 };
    push.commands[0] = drawn;
    push.commands[1] = none;
    // The first is drawn by no command: blue would show only if the draw ignored its first
    // instance.
    push.colors[0] = float4(0.0, 0.0, 1.0, 1.0);
    push.colors[1] = float4(1.0, 0.0, 0.0, 1.0);
    push.colors[2] = float4(0.0, 1.0, 0.0, 1.0);
    push.counts[0] = 2;
    push.counts[1] = 0;
    InterlockedAdd(push.counts[1], 7);
}

struct Varyings {
    float4 position : SV_Position;
    nointerpolation uint instance;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    // A small triangle round the center of pixel `instance - 1` of a row of two.
    float2 corners[3] = { float2(-0.4, -0.9), float2(0.4, -0.9), float2(0.0, 0.9) };
    Varyings out;
    out.position = float4(float(instance) - 1.5 + corners[index].x, corners[index].y, 0.0, 1.0);
    out.instance = instance;
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings in) : SV_Target0 {
    return push.colors[in.instance];
}
