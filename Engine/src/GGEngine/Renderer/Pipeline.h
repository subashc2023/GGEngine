#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"
#include <string>
#include <memory>
#include <vector>

namespace GGEngine {

    class Shader;
    class VertexLayout;
    class DescriptorSetLayout;

    // Pipeline blend mode presets
    enum class BlendMode
    {
        None,       // No blending (opaque)
        Alpha,      // Standard alpha blending
        Additive    // Additive blending
    };

    // Push constant range specification
    struct GG_API PushConstantRange
    {
        ShaderStage stageFlags = ShaderStage::AllGraphics;
        uint32_t offset = 0;
        uint32_t size = 0;
    };

    // Additional vertex binding for instanced rendering
    struct GG_API VertexBindingInfo
    {
        const VertexLayout* layout = nullptr;
        uint32_t binding = 0;
        uint32_t startLocation = 0;  // Starting attribute location
        VertexInputRate inputRate = VertexInputRate::Vertex;
    };

    // Pipeline specification for graphics pipelines
    struct GG_API PipelineSpecification
    {
        Shader* shader = nullptr;
        RHIRenderPassHandle renderPass;
        uint32_t subpass = 0;

        // Vertex input (primary binding 0)
        const VertexLayout* vertexLayout = nullptr;  // nullptr = no vertex input (hardcoded in shader)

        // Additional vertex bindings (for instanced rendering, etc.)
        std::vector<VertexBindingInfo> additionalVertexBindings;

        // Input assembly
        PrimitiveTopology topology = PrimitiveTopology::TriangleList;

        // Rasterization
        PolygonMode polygonMode = PolygonMode::Fill;
        CullMode cullMode = CullMode::None;
        FrontFace frontFace = FrontFace::Clockwise;
        float lineWidth = 1.0f;

        // Multisampling
        SampleCount samples = SampleCount::Count1;

        // Depth
        bool depthTestEnable = false;
        bool depthWriteEnable = false;
        CompareOp depthCompareOp = CompareOp::Less;

        // Blending
        BlendMode blendMode = BlendMode::None;

        // Push constants
        std::vector<PushConstantRange> pushConstantRanges;

        // Descriptor set layouts (in order: set 0, set 1, etc.)
        std::vector<RHIDescriptorSetLayoutHandle> descriptorSetLayouts;

        // Name for debugging
        std::string debugName;
    };

    class GG_API Pipeline
    {
    public:
        Pipeline(const PipelineSpecification& spec);
        ~Pipeline();

        Pipeline(const Pipeline&) = delete;
        Pipeline& operator=(const Pipeline&) = delete;

        // Bind to command buffer
        void Bind(RHICommandBufferHandle cmd);

        // Recreate pipeline (useful after shader hot-reload)
        void Recreate();

        RHIPipelineHandle GetHandle() const { return m_Handle; }
        RHIPipelineLayoutHandle GetLayoutHandle() const { return m_LayoutHandle; }
        const PipelineSpecification& GetSpecification() const { return m_Specification; }

    private:
        void Create();
        void Destroy();

        PipelineSpecification m_Specification;
        RHIPipelineHandle m_Handle;
        RHIPipelineLayoutHandle m_LayoutHandle;
    };

}
