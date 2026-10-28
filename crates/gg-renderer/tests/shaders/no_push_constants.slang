// A triangle that covers the view, whatever the mesh, in one color: it reads none of the push
// constants, so slangc leaves them out, and the contract allows that. It draws no entity, so its
// ID is 0.

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    float2 corner = float2((index << 1) & 2, index & 2);
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};

[shader("fragment")]
Targets fragment_main() {
    Targets out;
    out.color = float4(0.25, 0.5, 0.75, 1.0);
    out.id = uint2(0, 0);
    return out;
}
