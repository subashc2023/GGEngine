#include "ggpch.h"
#include "VulkanResourceRegistry.h"
#include "VulkanConversions.h"

namespace GGEngine {

    // ============================================================================
    // Vulkan Resource Registry Implementation
    // ============================================================================

    VulkanResourceRegistry& VulkanResourceRegistry::Get()
    {
        static VulkanResourceRegistry instance;
        return instance;
    }

    uint64_t VulkanResourceRegistry::GenerateId()
    {
        return m_NextId++;
    }

    // Pipeline
    RHIPipelineHandle VulkanResourceRegistry::RegisterPipeline(VkPipeline pipeline, VkPipelineLayout layout)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Pipelines[id] = { pipeline, layout };
        return RHIPipelineHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterPipeline(RHIPipelineHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Pipelines.erase(handle.id);
    }

    VulkanResourceRegistry::PipelineData VulkanResourceRegistry::GetPipelineData(RHIPipelineHandle handle) const
    {
        auto it = m_Pipelines.find(handle.id);
        if (it != m_Pipelines.end())
            return it->second;
        return {};
    }

    VkPipeline VulkanResourceRegistry::GetPipeline(RHIPipelineHandle handle) const
    {
        return GetPipelineData(handle).pipeline;
    }

    VkPipelineLayout VulkanResourceRegistry::GetPipelineLayout(RHIPipelineHandle handle) const
    {
        return GetPipelineData(handle).layout;
    }

    // Pipeline Layout
    RHIPipelineLayoutHandle VulkanResourceRegistry::RegisterPipelineLayout(VkPipelineLayout layout)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_PipelineLayouts[id] = layout;
        return RHIPipelineLayoutHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterPipelineLayout(RHIPipelineLayoutHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_PipelineLayouts.erase(handle.id);
    }

    VkPipelineLayout VulkanResourceRegistry::GetPipelineLayout(RHIPipelineLayoutHandle handle) const
    {
        auto it = m_PipelineLayouts.find(handle.id);
        if (it != m_PipelineLayouts.end())
            return it->second;
        return VK_NULL_HANDLE;
    }

    // Render Pass
    RHIRenderPassHandle VulkanResourceRegistry::RegisterRenderPass(VkRenderPass renderPass, VkFramebuffer framebuffer,
                                                                    uint32_t width, uint32_t height)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

        // Check if this render pass is already registered (idempotent)
        for (const auto& [id, data] : m_RenderPasses)
        {
            if (data.renderPass == renderPass)
            {
                // Update framebuffer info if provided (may change on resize)
                if (framebuffer != VK_NULL_HANDLE)
                {
                    m_RenderPasses[id].framebuffer = framebuffer;
                    m_RenderPasses[id].width = width;
                    m_RenderPasses[id].height = height;
                }
                return RHIRenderPassHandle{ id };
            }
        }

        // Not found, register new
        uint64_t id = GenerateId();
        m_RenderPasses[id] = { renderPass, framebuffer, width, height };
        return RHIRenderPassHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterRenderPass(RHIRenderPassHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_RenderPasses.erase(handle.id);
    }

    VulkanResourceRegistry::RenderPassData VulkanResourceRegistry::GetRenderPassData(RHIRenderPassHandle handle) const
    {
        auto it = m_RenderPasses.find(handle.id);
        if (it != m_RenderPasses.end())
            return it->second;
        return {};
    }

    VkRenderPass VulkanResourceRegistry::GetRenderPass(RHIRenderPassHandle handle) const
    {
        return GetRenderPassData(handle).renderPass;
    }

    VkFramebuffer VulkanResourceRegistry::GetFramebuffer(RHIRenderPassHandle handle) const
    {
        return GetRenderPassData(handle).framebuffer;
    }

    // Buffer
    RHIBufferHandle VulkanResourceRegistry::RegisterBuffer(VkBuffer buffer, VmaAllocation allocation,
                                                           uint64_t size, bool cpuVisible)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Buffers[id] = { buffer, allocation, size, cpuVisible };
        return RHIBufferHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterBuffer(RHIBufferHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Buffers.erase(handle.id);
    }

    VulkanResourceRegistry::BufferData VulkanResourceRegistry::GetBufferData(RHIBufferHandle handle) const
    {
        auto it = m_Buffers.find(handle.id);
        if (it != m_Buffers.end())
            return it->second;
        return {};
    }

    VkBuffer VulkanResourceRegistry::GetBuffer(RHIBufferHandle handle) const
    {
        return GetBufferData(handle).buffer;
    }

    VmaAllocation VulkanResourceRegistry::GetBufferAllocation(RHIBufferHandle handle) const
    {
        return GetBufferData(handle).allocation;
    }

    // Texture
    RHITextureHandle VulkanResourceRegistry::RegisterTexture(VkImage image, VkImageView view, VkSampler sampler,
                                                             VmaAllocation allocation, uint32_t width, uint32_t height,
                                                             TextureFormat format)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Textures[id] = { image, view, sampler, allocation, width, height, format };
        return RHITextureHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterTexture(RHITextureHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Textures.erase(handle.id);
    }

    VulkanResourceRegistry::TextureData VulkanResourceRegistry::GetTextureData(RHITextureHandle handle) const
    {
        auto it = m_Textures.find(handle.id);
        if (it != m_Textures.end())
            return it->second;
        return {};
    }

    VkImage VulkanResourceRegistry::GetTextureImage(RHITextureHandle handle) const
    {
        return GetTextureData(handle).image;
    }

    VkImageView VulkanResourceRegistry::GetTextureView(RHITextureHandle handle) const
    {
        return GetTextureData(handle).imageView;
    }

    VkSampler VulkanResourceRegistry::GetTextureSampler(RHITextureHandle handle) const
    {
        return GetTextureData(handle).sampler;
    }

    // Shader Module (individual stages)
    RHIShaderModuleHandle VulkanResourceRegistry::RegisterShaderModule(VkShaderModule module, ShaderStage stage, const std::string& entryPoint)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_ShaderModules[id] = { module, stage, entryPoint };
        return RHIShaderModuleHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterShaderModule(RHIShaderModuleHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_ShaderModules.erase(handle.id);
    }

    VulkanResourceRegistry::ShaderModuleData VulkanResourceRegistry::GetShaderModuleData(RHIShaderModuleHandle handle) const
    {
        auto it = m_ShaderModules.find(handle.id);
        if (it != m_ShaderModules.end())
            return it->second;
        return {};
    }

    VkShaderModule VulkanResourceRegistry::GetShaderModule(RHIShaderModuleHandle handle) const
    {
        return GetShaderModuleData(handle).module;
    }

    // Shader Program (collection of modules)
    RHIShaderHandle VulkanResourceRegistry::RegisterShader(const std::vector<RHIShaderModuleHandle>& moduleHandles)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Shaders[id] = { moduleHandles };
        return RHIShaderHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterShader(RHIShaderHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Shaders.erase(handle.id);
    }

    VulkanResourceRegistry::ShaderData VulkanResourceRegistry::GetShaderData(RHIShaderHandle handle) const
    {
        auto it = m_Shaders.find(handle.id);
        if (it != m_Shaders.end())
            return it->second;
        return {};
    }

    std::vector<VkPipelineShaderStageCreateInfo> VulkanResourceRegistry::GetShaderPipelineStageCreateInfos(RHIShaderHandle handle) const
    {
        std::vector<VkPipelineShaderStageCreateInfo> infos;
        auto data = GetShaderData(handle);
        infos.reserve(data.moduleHandles.size());

        for (const auto& moduleHandle : data.moduleHandles)
        {
            auto moduleData = GetShaderModuleData(moduleHandle);
            if (moduleData.module == VK_NULL_HANDLE)
                continue;

            VkPipelineShaderStageCreateInfo info{};
            info.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
            info.stage = static_cast<VkShaderStageFlagBits>(ToVulkan(moduleData.stage));
            info.module = moduleData.module;
            info.pName = moduleData.entryPoint.c_str();
            infos.push_back(info);
        }

        return infos;
    }

    // Descriptor Set Layout
    RHIDescriptorSetLayoutHandle VulkanResourceRegistry::RegisterDescriptorSetLayout(VkDescriptorSetLayout layout)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_DescriptorSetLayouts[id] = layout;
        return RHIDescriptorSetLayoutHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_DescriptorSetLayouts.erase(handle.id);
    }

    VkDescriptorSetLayout VulkanResourceRegistry::GetDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle) const
    {
        auto it = m_DescriptorSetLayouts.find(handle.id);
        if (it != m_DescriptorSetLayouts.end())
            return it->second;
        return VK_NULL_HANDLE;
    }

    // Descriptor Set
    RHIDescriptorSetHandle VulkanResourceRegistry::RegisterDescriptorSet(VkDescriptorSet set,
                                                                          RHIDescriptorSetLayoutHandle layoutHandle,
                                                                          VkDescriptorPool owningPool)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_DescriptorSets[id] = { set, layoutHandle, owningPool };
        return RHIDescriptorSetHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterDescriptorSet(RHIDescriptorSetHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_DescriptorSets.erase(handle.id);
    }

    VulkanResourceRegistry::DescriptorSetData VulkanResourceRegistry::GetDescriptorSetData(RHIDescriptorSetHandle handle) const
    {
        auto it = m_DescriptorSets.find(handle.id);
        if (it != m_DescriptorSets.end())
            return it->second;
        return {};
    }

    VkDescriptorSet VulkanResourceRegistry::GetDescriptorSet(RHIDescriptorSetHandle handle) const
    {
        return GetDescriptorSetData(handle).descriptorSet;
    }

    // Command Buffer
    void VulkanResourceRegistry::SetCurrentCommandBuffer(uint32_t frameIndex, VkCommandBuffer cmd)
    {
        if (frameIndex < MaxFramesInFlight)
        {
            m_CommandBuffers[frameIndex] = cmd;
            // Generate a unique handle ID for this frame's command buffer
            // We use a fixed offset to distinguish command buffer handles
            m_CommandBufferHandleIds[frameIndex] = 0xFFFF0000 + frameIndex;
        }
    }

    RHICommandBufferHandle VulkanResourceRegistry::GetCurrentCommandBufferHandle(uint32_t frameIndex) const
    {
        if (frameIndex < MaxFramesInFlight)
            return RHICommandBufferHandle{ m_CommandBufferHandleIds[frameIndex] };
        return NullCommandBuffer;
    }

    VkCommandBuffer VulkanResourceRegistry::GetCommandBuffer(RHICommandBufferHandle handle) const
    {
        // Check if it's the immediate command buffer
        if (handle.id == ImmediateCommandBufferHandleId)
            return m_ImmediateCommandBuffer;

        // Check if it's a frame command buffer
        for (uint32_t i = 0; i < MaxFramesInFlight; ++i)
        {
            if (m_CommandBufferHandleIds[i] == handle.id)
                return m_CommandBuffers[i];
        }
        return VK_NULL_HANDLE;
    }

    void VulkanResourceRegistry::SetImmediateCommandBuffer(VkCommandBuffer cmd)
    {
        m_ImmediateCommandBuffer = cmd;
    }

    RHICommandBufferHandle VulkanResourceRegistry::GetImmediateCommandBufferHandle() const
    {
        return RHICommandBufferHandle{ ImmediateCommandBufferHandleId };
    }

    void VulkanResourceRegistry::Clear()
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Pipelines.clear();
        m_PipelineLayouts.clear();
        m_RenderPasses.clear();
        m_Buffers.clear();
        m_Textures.clear();
        m_ShaderModules.clear();
        m_Shaders.clear();
        m_DescriptorSetLayouts.clear();
        m_DescriptorSets.clear();
    }

}
