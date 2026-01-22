#include "ggpch.h"
#include "Pipeline.h"
#include "VertexLayout.h"
#include "DescriptorSet.h"
#include "GGEngine/Asset/Shader.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "Platform/Vulkan/VulkanUtils.h"
#include "Platform/Vulkan/VulkanRHI.h"

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

    void Pipeline::Bind(RHICommandBufferHandle cmd)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipeline pipeline = registry.GetPipeline(m_Handle);
        vkCmdBindPipeline(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
    }

    void Pipeline::BindVk(void* vkCmd)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkPipeline pipeline = registry.GetPipeline(m_Handle);
        vkCmdBindPipeline(static_cast<VkCommandBuffer>(vkCmd), VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
    }

    void Pipeline::Create()
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        if (!m_Specification.shader || !m_Specification.shader->IsLoaded())
        {
            GG_CORE_ERROR("Pipeline creation failed: Invalid or unloaded shader");
            return;
        }

        if (!m_Specification.renderPass.IsValid())
        {
            GG_CORE_ERROR("Pipeline creation failed: No render pass specified");
            return;
        }

        // Get the actual VkRenderPass from the registry
        VkRenderPass vkRenderPass = registry.GetRenderPass(m_Specification.renderPass);
        if (vkRenderPass == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("Pipeline creation failed: Invalid render pass handle");
            return;
        }

        // Build shader stage create infos from shader's stages
        const auto& stages = m_Specification.shader->GetStages();
        if (stages.empty())
        {
            GG_CORE_ERROR("Pipeline creation failed: Shader has no stages");
            return;
        }

        std::vector<VkPipelineShaderStageCreateInfo> shaderStages;
        shaderStages.reserve(stages.size());
        for (const auto& stageInfo : stages)
        {
            VkShaderModule module = registry.GetShaderModule(stageInfo.handle);
            if (module == VK_NULL_HANDLE)
            {
                GG_CORE_ERROR("Pipeline creation failed: Invalid shader module for stage {}", static_cast<uint32_t>(stageInfo.stage));
                return;
            }

            VkPipelineShaderStageCreateInfo createInfo{};
            createInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
            createInfo.stage = static_cast<VkShaderStageFlagBits>(ToVulkan(stageInfo.stage));
            createInfo.module = module;
            createInfo.pName = stageInfo.entryPoint.c_str();
            shaderStages.push_back(createInfo);
        }

        // Vertex input - use layout if provided, otherwise empty (hardcoded in shader)
        VkPipelineVertexInputStateCreateInfo vertexInputInfo{};
        vertexInputInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO;

        VkVertexInputBindingDescription bindingDesc{};
        std::vector<VkVertexInputAttributeDescription> attributeDescs;

        if (m_Specification.vertexLayout && !m_Specification.vertexLayout->IsEmpty())
        {
            // Convert RHI descriptions to Vulkan
            bindingDesc = ToVulkan(m_Specification.vertexLayout->GetBindingDescription());
            const auto rhiAttribs = m_Specification.vertexLayout->GetAttributeDescriptions();
            attributeDescs.reserve(rhiAttribs.size());
            for (const auto& attrib : rhiAttribs)
            {
                attributeDescs.push_back(ToVulkan(attrib));
            }

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
        inputAssembly.topology = ToVulkan(m_Specification.topology);
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
        rasterizer.polygonMode = ToVulkan(m_Specification.polygonMode);
        rasterizer.lineWidth = m_Specification.lineWidth;
        rasterizer.cullMode = ToVulkan(m_Specification.cullMode);
        rasterizer.frontFace = ToVulkan(m_Specification.frontFace);
        rasterizer.depthBiasEnable = VK_FALSE;

        // Multisampling
        VkPipelineMultisampleStateCreateInfo multisampling{};
        multisampling.sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO;
        multisampling.sampleShadingEnable = VK_FALSE;
        multisampling.rasterizationSamples = ToVulkan(m_Specification.samples);

        // Depth stencil
        VkPipelineDepthStencilStateCreateInfo depthStencil{};
        depthStencil.sType = VK_STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO;
        depthStencil.depthTestEnable = m_Specification.depthTestEnable ? VK_TRUE : VK_FALSE;
        depthStencil.depthWriteEnable = m_Specification.depthWriteEnable ? VK_TRUE : VK_FALSE;
        depthStencil.depthCompareOp = ToVulkan(m_Specification.depthCompareOp);
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
            vkRange.stageFlags = ToVulkan(range.stageFlags);
            vkRange.offset = range.offset;
            vkRange.size = range.size;
            vkPushConstantRanges.push_back(vkRange);
        }

        // Convert descriptor set layout handles to VkDescriptorSetLayouts
        std::vector<VkDescriptorSetLayout> vkSetLayouts;
        for (const auto& layoutHandle : m_Specification.descriptorSetLayouts)
        {
            VkDescriptorSetLayout vkLayout = registry.GetDescriptorSetLayout(layoutHandle);
            vkSetLayouts.push_back(vkLayout);
        }

        // Pipeline layout
        VkPipelineLayoutCreateInfo pipelineLayoutInfo{};
        pipelineLayoutInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO;
        pipelineLayoutInfo.setLayoutCount = static_cast<uint32_t>(vkSetLayouts.size());
        pipelineLayoutInfo.pSetLayouts = vkSetLayouts.empty() ? nullptr : vkSetLayouts.data();
        pipelineLayoutInfo.pushConstantRangeCount = static_cast<uint32_t>(vkPushConstantRanges.size());
        pipelineLayoutInfo.pPushConstantRanges = vkPushConstantRanges.empty() ? nullptr : vkPushConstantRanges.data();

        VkPipelineLayout pipelineLayout = VK_NULL_HANDLE;
        VK_CHECK_RETURN(
            vkCreatePipelineLayout(device, &pipelineLayoutInfo, nullptr, &pipelineLayout),
            "Failed to create pipeline layout");

        // Register pipeline layout
        m_LayoutHandle = registry.RegisterPipelineLayout(pipelineLayout);

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
        pipelineInfo.layout = pipelineLayout;
        pipelineInfo.renderPass = vkRenderPass;
        pipelineInfo.subpass = m_Specification.subpass;
        pipelineInfo.basePipelineHandle = VK_NULL_HANDLE;

        VkPipeline pipeline = VK_NULL_HANDLE;
        VK_CHECK_RETURN(
            vkCreateGraphicsPipelines(device, VK_NULL_HANDLE, 1, &pipelineInfo, nullptr, &pipeline),
            "Failed to create graphics pipeline");

        // Register pipeline (stores both pipeline and layout)
        m_Handle = registry.RegisterPipeline(pipeline, pipelineLayout);

        if (!m_Specification.debugName.empty())
        {
            GG_CORE_INFO("Pipeline '{}' created successfully", m_Specification.debugName);
        }
    }

    void Pipeline::Destroy()
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();
        vkDeviceWaitIdle(device);

        if (m_Handle.IsValid())
        {
            VkPipeline pipeline = registry.GetPipeline(m_Handle);
            if (pipeline != VK_NULL_HANDLE)
            {
                vkDestroyPipeline(device, pipeline, nullptr);
            }
            registry.UnregisterPipeline(m_Handle);
            m_Handle = NullPipeline;
        }

        if (m_LayoutHandle.IsValid())
        {
            VkPipelineLayout layout = registry.GetPipelineLayout(m_LayoutHandle);
            if (layout != VK_NULL_HANDLE)
            {
                vkDestroyPipelineLayout(device, layout, nullptr);
            }
            registry.UnregisterPipelineLayout(m_LayoutHandle);
            m_LayoutHandle = NullPipelineLayout;
        }
    }

}
