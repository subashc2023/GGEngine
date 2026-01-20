#include "ggpch.h"
#include "Pipeline.h"
#include "VertexLayout.h"
#include "GGEngine/Asset/Shader.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Log.h"

namespace GGEngine {

    Pipeline::Pipeline(const PipelineSpecification& spec)
        : m_Specification(spec)
    {
        Create();
    }

    Pipeline::~Pipeline()
    {
        Destroy();
    }

    void Pipeline::Bind(VkCommandBuffer cmd)
    {
        vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, m_Pipeline);
    }

    void Pipeline::Create()
    {
        auto device = VulkanContext::Get().GetDevice();

        if (!m_Specification.shader || !m_Specification.shader->IsLoaded())
        {
            GG_CORE_ERROR("Pipeline creation failed: Invalid or unloaded shader");
            return;
        }

        if (m_Specification.renderPass == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("Pipeline creation failed: No render pass specified");
            return;
        }

        auto shaderStages = m_Specification.shader->GetPipelineStageCreateInfos();
        if (shaderStages.empty())
        {
            GG_CORE_ERROR("Pipeline creation failed: Shader has no stages");
            return;
        }

        // Vertex input - use layout if provided, otherwise empty (hardcoded in shader)
        VkPipelineVertexInputStateCreateInfo vertexInputInfo{};
        vertexInputInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO;

        VkVertexInputBindingDescription bindingDesc{};
        std::vector<VkVertexInputAttributeDescription> attributeDescs;

        if (m_Specification.vertexLayout && !m_Specification.vertexLayout->IsEmpty())
        {
            bindingDesc = m_Specification.vertexLayout->GetBindingDescription();
            attributeDescs = m_Specification.vertexLayout->GetAttributeDescriptions();

            vertexInputInfo.vertexBindingDescriptionCount = 1;
            vertexInputInfo.pVertexBindingDescriptions = &bindingDesc;
            vertexInputInfo.vertexAttributeDescriptionCount = static_cast<uint32_t>(attributeDescs.size());
            vertexInputInfo.pVertexAttributeDescriptions = attributeDescs.data();
        }
        else
        {
            vertexInputInfo.vertexBindingDescriptionCount = 0;
            vertexInputInfo.vertexAttributeDescriptionCount = 0;
        }

        // Input assembly
        VkPipelineInputAssemblyStateCreateInfo inputAssembly{};
        inputAssembly.sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO;
        inputAssembly.topology = m_Specification.topology;
        inputAssembly.primitiveRestartEnable = VK_FALSE;

        // Dynamic viewport/scissor
        VkDynamicState dynamicStates[] = { VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR };
        VkPipelineDynamicStateCreateInfo dynamicState{};
        dynamicState.sType = VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO;
        dynamicState.dynamicStateCount = 2;
        dynamicState.pDynamicStates = dynamicStates;

        VkPipelineViewportStateCreateInfo viewportState{};
        viewportState.sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO;
        viewportState.viewportCount = 1;
        viewportState.scissorCount = 1;

        // Rasterizer
        VkPipelineRasterizationStateCreateInfo rasterizer{};
        rasterizer.sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO;
        rasterizer.depthClampEnable = VK_FALSE;
        rasterizer.rasterizerDiscardEnable = VK_FALSE;
        rasterizer.polygonMode = m_Specification.polygonMode;
        rasterizer.lineWidth = m_Specification.lineWidth;
        rasterizer.cullMode = m_Specification.cullMode;
        rasterizer.frontFace = m_Specification.frontFace;
        rasterizer.depthBiasEnable = VK_FALSE;

        // Multisampling
        VkPipelineMultisampleStateCreateInfo multisampling{};
        multisampling.sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO;
        multisampling.sampleShadingEnable = VK_FALSE;
        multisampling.rasterizationSamples = m_Specification.samples;

        // Depth stencil
        VkPipelineDepthStencilStateCreateInfo depthStencil{};
        depthStencil.sType = VK_STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO;
        depthStencil.depthTestEnable = m_Specification.depthTestEnable ? VK_TRUE : VK_FALSE;
        depthStencil.depthWriteEnable = m_Specification.depthWriteEnable ? VK_TRUE : VK_FALSE;
        depthStencil.depthCompareOp = m_Specification.depthCompareOp;
        depthStencil.depthBoundsTestEnable = VK_FALSE;
        depthStencil.stencilTestEnable = VK_FALSE;

        // Color blending
        VkPipelineColorBlendAttachmentState colorBlendAttachment{};
        colorBlendAttachment.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                                              VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;

        switch (m_Specification.blendMode)
        {
            case BlendMode::None:
                colorBlendAttachment.blendEnable = VK_FALSE;
                break;

            case BlendMode::Alpha:
                colorBlendAttachment.blendEnable = VK_TRUE;
                colorBlendAttachment.srcColorBlendFactor = VK_BLEND_FACTOR_SRC_ALPHA;
                colorBlendAttachment.dstColorBlendFactor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA;
                colorBlendAttachment.colorBlendOp = VK_BLEND_OP_ADD;
                colorBlendAttachment.srcAlphaBlendFactor = VK_BLEND_FACTOR_ONE;
                colorBlendAttachment.dstAlphaBlendFactor = VK_BLEND_FACTOR_ZERO;
                colorBlendAttachment.alphaBlendOp = VK_BLEND_OP_ADD;
                break;

            case BlendMode::Additive:
                colorBlendAttachment.blendEnable = VK_TRUE;
                colorBlendAttachment.srcColorBlendFactor = VK_BLEND_FACTOR_SRC_ALPHA;
                colorBlendAttachment.dstColorBlendFactor = VK_BLEND_FACTOR_ONE;
                colorBlendAttachment.colorBlendOp = VK_BLEND_OP_ADD;
                colorBlendAttachment.srcAlphaBlendFactor = VK_BLEND_FACTOR_ONE;
                colorBlendAttachment.dstAlphaBlendFactor = VK_BLEND_FACTOR_ZERO;
                colorBlendAttachment.alphaBlendOp = VK_BLEND_OP_ADD;
                break;
        }

        VkPipelineColorBlendStateCreateInfo colorBlending{};
        colorBlending.sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO;
        colorBlending.logicOpEnable = VK_FALSE;
        colorBlending.attachmentCount = 1;
        colorBlending.pAttachments = &colorBlendAttachment;

        // Push constant ranges
        std::vector<VkPushConstantRange> vkPushConstantRanges;
        for (const auto& range : m_Specification.pushConstantRanges)
        {
            VkPushConstantRange vkRange{};
            vkRange.stageFlags = range.stageFlags;
            vkRange.offset = range.offset;
            vkRange.size = range.size;
            vkPushConstantRanges.push_back(vkRange);
        }

        // Pipeline layout
        VkPipelineLayoutCreateInfo pipelineLayoutInfo{};
        pipelineLayoutInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO;
        pipelineLayoutInfo.setLayoutCount = static_cast<uint32_t>(m_Specification.descriptorSetLayouts.size());
        pipelineLayoutInfo.pSetLayouts = m_Specification.descriptorSetLayouts.empty() ? nullptr : m_Specification.descriptorSetLayouts.data();
        pipelineLayoutInfo.pushConstantRangeCount = static_cast<uint32_t>(vkPushConstantRanges.size());
        pipelineLayoutInfo.pPushConstantRanges = vkPushConstantRanges.empty() ? nullptr : vkPushConstantRanges.data();

        if (vkCreatePipelineLayout(device, &pipelineLayoutInfo, nullptr, &m_PipelineLayout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create pipeline layout!");
            return;
        }

        // Create pipeline
        VkGraphicsPipelineCreateInfo pipelineInfo{};
        pipelineInfo.sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO;
        pipelineInfo.stageCount = static_cast<uint32_t>(shaderStages.size());
        pipelineInfo.pStages = shaderStages.data();
        pipelineInfo.pVertexInputState = &vertexInputInfo;
        pipelineInfo.pInputAssemblyState = &inputAssembly;
        pipelineInfo.pViewportState = &viewportState;
        pipelineInfo.pRasterizationState = &rasterizer;
        pipelineInfo.pMultisampleState = &multisampling;
        pipelineInfo.pDepthStencilState = m_Specification.depthTestEnable ? &depthStencil : nullptr;
        pipelineInfo.pColorBlendState = &colorBlending;
        pipelineInfo.pDynamicState = &dynamicState;
        pipelineInfo.layout = m_PipelineLayout;
        pipelineInfo.renderPass = m_Specification.renderPass;
        pipelineInfo.subpass = m_Specification.subpass;
        pipelineInfo.basePipelineHandle = VK_NULL_HANDLE;

        if (vkCreateGraphicsPipelines(device, VK_NULL_HANDLE, 1, &pipelineInfo, nullptr, &m_Pipeline) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create graphics pipeline!");
            return;
        }

        if (!m_Specification.debugName.empty())
        {
            GG_CORE_INFO("Pipeline '{}' created successfully", m_Specification.debugName);
        }
    }

    void Pipeline::Destroy()
    {
        auto device = VulkanContext::Get().GetDevice();
        vkDeviceWaitIdle(device);

        if (m_Pipeline != VK_NULL_HANDLE)
        {
            vkDestroyPipeline(device, m_Pipeline, nullptr);
            m_Pipeline = VK_NULL_HANDLE;
        }

        if (m_PipelineLayout != VK_NULL_HANDLE)
        {
            vkDestroyPipelineLayout(device, m_PipelineLayout, nullptr);
            m_PipelineLayout = VK_NULL_HANDLE;
        }
    }

}
