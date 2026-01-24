#include "ggpch.h"
#include "VulkanResourceRegistry.h"
#include "VulkanConversions.h"
#include "VulkanContext.h"
#include "VulkanUtils.h"
#include "GGEngine/RHI/RHIDevice.h"

namespace GGEngine {

    // ============================================================================
    // Pipeline Management
    // ============================================================================

    Result<RHIGraphicsPipelineResult> RHIDevice::TryCreateGraphicsPipeline(const RHIGraphicsPipelineSpecification& spec)
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        // Shader stages
        // Store module data to keep entry point strings alive until after vkCreateGraphicsPipelines
        std::vector<VulkanResourceRegistry::ShaderModuleData> moduleDataStorage;
        moduleDataStorage.reserve(spec.shaderModules.size());
        for (const auto& moduleHandle : spec.shaderModules)
        {
            auto moduleData = registry.GetShaderModuleData(moduleHandle);
            if (moduleData.module != VK_NULL_HANDLE)
                moduleDataStorage.push_back(moduleData);
        }

        if (moduleDataStorage.empty())
            return Result<RHIGraphicsPipelineResult>::Err("No valid shader modules provided");

        std::vector<VkPipelineShaderStageCreateInfo> shaderStages;
        shaderStages.reserve(moduleDataStorage.size());
        for (const auto& moduleData : moduleDataStorage)
        {
            VkPipelineShaderStageCreateInfo stageInfo{};
            stageInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
            stageInfo.stage = static_cast<VkShaderStageFlagBits>(ToVulkan(moduleData.stage));
            stageInfo.module = moduleData.module;
            stageInfo.pName = moduleData.entryPoint.c_str();
            shaderStages.push_back(stageInfo);
        }

        // Vertex input
        std::vector<VkVertexInputBindingDescription> vkBindings;
        std::vector<VkVertexInputAttributeDescription> vkAttributes;
        for (const auto& binding : spec.vertexBindings)
            vkBindings.push_back(ToVulkan(binding));
        for (const auto& attr : spec.vertexAttributes)
            vkAttributes.push_back(ToVulkan(attr));

        VkPipelineVertexInputStateCreateInfo vertexInputInfo{};
        vertexInputInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO;
        vertexInputInfo.vertexBindingDescriptionCount = static_cast<uint32_t>(vkBindings.size());
        vertexInputInfo.pVertexBindingDescriptions = vkBindings.data();
        vertexInputInfo.vertexAttributeDescriptionCount = static_cast<uint32_t>(vkAttributes.size());
        vertexInputInfo.pVertexAttributeDescriptions = vkAttributes.data();

        // Input assembly
        VkPipelineInputAssemblyStateCreateInfo inputAssembly{};
        inputAssembly.sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO;
        inputAssembly.topology = ToVulkan(spec.topology);
        inputAssembly.primitiveRestartEnable = VK_FALSE;

        // Viewport state (dynamic)
        VkPipelineViewportStateCreateInfo viewportState{};
        viewportState.sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO;
        viewportState.viewportCount = 1;
        viewportState.scissorCount = 1;

        // Rasterization
        VkPipelineRasterizationStateCreateInfo rasterizer{};
        rasterizer.sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO;
        rasterizer.depthClampEnable = VK_FALSE;
        rasterizer.rasterizerDiscardEnable = VK_FALSE;
        rasterizer.polygonMode = ToVulkan(spec.polygonMode);
        rasterizer.cullMode = ToVulkan(spec.cullMode);
        rasterizer.frontFace = ToVulkan(spec.frontFace);
        rasterizer.lineWidth = spec.lineWidth;

        // Multisampling
        VkPipelineMultisampleStateCreateInfo multisampling{};
        multisampling.sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO;
        multisampling.sampleShadingEnable = VK_FALSE;
        multisampling.rasterizationSamples = ToVulkan(spec.samples);

        // Depth stencil
        VkPipelineDepthStencilStateCreateInfo depthStencil{};
        depthStencil.sType = VK_STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO;
        depthStencil.depthTestEnable = spec.depthTestEnable ? VK_TRUE : VK_FALSE;
        depthStencil.depthWriteEnable = spec.depthWriteEnable ? VK_TRUE : VK_FALSE;
        depthStencil.depthCompareOp = ToVulkan(spec.depthCompareOp);

        // Color blending
        std::vector<VkPipelineColorBlendAttachmentState> colorBlendAttachments;
        for (const auto& blend : spec.colorBlendStates)
        {
            VkPipelineColorBlendAttachmentState attachment{};
            attachment.blendEnable = blend.enable ? VK_TRUE : VK_FALSE;
            attachment.srcColorBlendFactor = ToVulkan(blend.srcColorFactor);
            attachment.dstColorBlendFactor = ToVulkan(blend.dstColorFactor);
            attachment.colorBlendOp = ToVulkan(blend.colorOp);
            attachment.srcAlphaBlendFactor = ToVulkan(blend.srcAlphaFactor);
            attachment.dstAlphaBlendFactor = ToVulkan(blend.dstAlphaFactor);
            attachment.alphaBlendOp = ToVulkan(blend.alphaOp);
            attachment.colorWriteMask = ToVulkan(blend.colorWriteMask);
            colorBlendAttachments.push_back(attachment);
        }
        if (colorBlendAttachments.empty())
        {
            VkPipelineColorBlendAttachmentState attachment{};
            attachment.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                                        VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
            colorBlendAttachments.push_back(attachment);
        }

        VkPipelineColorBlendStateCreateInfo colorBlending{};
        colorBlending.sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO;
        colorBlending.logicOpEnable = VK_FALSE;
        colorBlending.attachmentCount = static_cast<uint32_t>(colorBlendAttachments.size());
        colorBlending.pAttachments = colorBlendAttachments.data();

        // Dynamic state
        std::vector<VkDynamicState> dynamicStates = { VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR };
        VkPipelineDynamicStateCreateInfo dynamicState{};
        dynamicState.sType = VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO;
        dynamicState.dynamicStateCount = static_cast<uint32_t>(dynamicStates.size());
        dynamicState.pDynamicStates = dynamicStates.data();

        // Pipeline layout
        std::vector<VkDescriptorSetLayout> vkSetLayouts;
        for (const auto& layoutHandle : spec.descriptorSetLayouts)
            vkSetLayouts.push_back(registry.GetDescriptorSetLayout(layoutHandle));

        std::vector<VkPushConstantRange> vkPushConstants;
        for (const auto& pc : spec.pushConstantRanges)
        {
            VkPushConstantRange range{};
            range.stageFlags = ToVulkan(pc.stages);
            range.offset = pc.offset;
            range.size = pc.size;
            vkPushConstants.push_back(range);
        }

        VkPipelineLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO;
        layoutInfo.setLayoutCount = static_cast<uint32_t>(vkSetLayouts.size());
        layoutInfo.pSetLayouts = vkSetLayouts.data();
        layoutInfo.pushConstantRangeCount = static_cast<uint32_t>(vkPushConstants.size());
        layoutInfo.pPushConstantRanges = vkPushConstants.data();

        VkPipelineLayout pipelineLayout = VK_NULL_HANDLE;
        VkResult vkResult = vkCreatePipelineLayout(device, &layoutInfo, nullptr, &pipelineLayout);
        if (vkResult != VK_SUCCESS)
        {
            return Result<RHIGraphicsPipelineResult>::Err(
                std::string("vkCreatePipelineLayout failed: ") + std::string(VkResultToString(vkResult)));
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
        pipelineInfo.pDepthStencilState = &depthStencil;
        pipelineInfo.pColorBlendState = &colorBlending;
        pipelineInfo.pDynamicState = &dynamicState;
        pipelineInfo.layout = pipelineLayout;
        pipelineInfo.renderPass = registry.GetRenderPass(spec.renderPass);
        pipelineInfo.subpass = spec.subpass;

        VkPipeline pipeline = VK_NULL_HANDLE;
        vkResult = vkCreateGraphicsPipelines(device, VK_NULL_HANDLE, 1, &pipelineInfo, nullptr, &pipeline);
        if (vkResult != VK_SUCCESS)
        {
            vkDestroyPipelineLayout(device, pipelineLayout, nullptr);
            return Result<RHIGraphicsPipelineResult>::Err(
                std::string("vkCreateGraphicsPipelines failed: ") + std::string(VkResultToString(vkResult)));
        }

        RHIGraphicsPipelineResult result;
        result.pipeline = registry.RegisterPipeline(pipeline, pipelineLayout);
        result.layout = registry.RegisterPipelineLayout(pipelineLayout);
        return Result<RHIGraphicsPipelineResult>::Ok(result);
    }

    RHIGraphicsPipelineResult RHIDevice::CreateGraphicsPipeline(const RHIGraphicsPipelineSpecification& spec)
    {
        auto result = TryCreateGraphicsPipeline(spec);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateGraphicsPipeline: {}", result.Error());
            return RHIGraphicsPipelineResult{};
        }
        return result.Value();
    }

    void RHIDevice::DestroyPipeline(RHIPipelineHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto pipelineData = registry.GetPipelineData(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (pipelineData.pipeline != VK_NULL_HANDLE)
            vkDestroyPipeline(device, pipelineData.pipeline, nullptr);
        // Note: layout is destroyed separately via DestroyPipelineLayout

        registry.UnregisterPipeline(handle);
    }

    void RHIDevice::DestroyPipelineLayout(RHIPipelineLayoutHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        VkPipelineLayout layout = registry.GetPipelineLayout(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (layout != VK_NULL_HANDLE)
            vkDestroyPipelineLayout(device, layout, nullptr);

        registry.UnregisterPipelineLayout(handle);
    }

    RHIPipelineLayoutHandle RHIDevice::GetPipelineLayout(RHIPipelineHandle pipeline) const
    {
        if (!pipeline.IsValid()) return NullPipelineLayout;

        auto& registry = VulkanResourceRegistry::Get();
        auto pipelineData = registry.GetPipelineData(pipeline);

        // Find the layout handle for this pipeline's layout
        // Since RegisterPipeline stores the layout, we need to look it up
        // For now, return a handle with the layout pointer as ID
        return RHIPipelineLayoutHandle{ reinterpret_cast<uint64_t>(pipelineData.layout) };
    }

}
