#pragma once

#include "GGEngine/Core/Core.h"
#include <vulkan/vulkan.h>
#include <string>
#include <memory>
#include <vector>

namespace GGEngine {

    class Shader;
    class VertexLayout;

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
        VkShaderStageFlags stageFlags = VK_SHADER_STAGE_ALL_GRAPHICS;
        uint32_t offset = 0;
        uint32_t size = 0;
    };

    // Pipeline specification for graphics pipelines
    struct GG_API PipelineSpecification
    {
        Shader* shader = nullptr;
        VkRenderPass renderPass = VK_NULL_HANDLE;
        uint32_t subpass = 0;

        // Vertex input
        const VertexLayout* vertexLayout = nullptr;  // nullptr = no vertex input (hardcoded in shader)

        // Input assembly
        VkPrimitiveTopology topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;

        // Rasterization
        VkPolygonMode polygonMode = VK_POLYGON_MODE_FILL;
        VkCullModeFlags cullMode = VK_CULL_MODE_NONE;
        VkFrontFace frontFace = VK_FRONT_FACE_CLOCKWISE;
        float lineWidth = 1.0f;

        // Multisampling
        VkSampleCountFlagBits samples = VK_SAMPLE_COUNT_1_BIT;

        // Depth
        bool depthTestEnable = false;
        bool depthWriteEnable = false;
        VkCompareOp depthCompareOp = VK_COMPARE_OP_LESS;

        // Blending
        BlendMode blendMode = BlendMode::None;

        // Push constants
        std::vector<PushConstantRange> pushConstantRanges;

        // Descriptor set layouts (in order: set 0, set 1, etc.)
        std::vector<VkDescriptorSetLayout> descriptorSetLayouts;

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

        void Bind(VkCommandBuffer cmd);

        VkPipeline GetVkPipeline() const { return m_Pipeline; }
        VkPipelineLayout GetLayout() const { return m_PipelineLayout; }
        const PipelineSpecification& GetSpecification() const { return m_Specification; }

    private:
        void Create();
        void Destroy();

        PipelineSpecification m_Specification;
        VkPipeline m_Pipeline = VK_NULL_HANDLE;
        VkPipelineLayout m_PipelineLayout = VK_NULL_HANDLE;
    };

}
