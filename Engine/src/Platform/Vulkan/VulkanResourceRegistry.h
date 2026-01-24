#pragma once

#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"

#include <vulkan/vulkan.h>
#include <vk_mem_alloc.h>
#include <unordered_map>
#include <mutex>
#include <vector>
#include <string>

namespace GGEngine {

    // ============================================================================
    // Vulkan Resource Registry
    // ============================================================================
    // Maps opaque RHI handles to actual Vulkan objects.
    // Thread-safe for registration/unregistration (lookups are fast but not locked).

    class GG_API VulkanResourceRegistry
    {
    public:
        static VulkanResourceRegistry& Get();

        // ========================================================================
        // Pipeline Management
        // ========================================================================
        struct PipelineData
        {
            VkPipeline pipeline = VK_NULL_HANDLE;
            VkPipelineLayout layout = VK_NULL_HANDLE;
        };

        RHIPipelineHandle RegisterPipeline(VkPipeline pipeline, VkPipelineLayout layout);
        void UnregisterPipeline(RHIPipelineHandle handle);
        PipelineData GetPipelineData(RHIPipelineHandle handle) const;
        VkPipeline GetPipeline(RHIPipelineHandle handle) const;
        VkPipelineLayout GetPipelineLayout(RHIPipelineHandle handle) const;

        // ========================================================================
        // Pipeline Layout Management
        // ========================================================================
        RHIPipelineLayoutHandle RegisterPipelineLayout(VkPipelineLayout layout);
        void UnregisterPipelineLayout(RHIPipelineLayoutHandle handle);
        VkPipelineLayout GetPipelineLayout(RHIPipelineLayoutHandle handle) const;

        // ========================================================================
        // Render Pass Management
        // ========================================================================
        struct RenderPassData
        {
            VkRenderPass renderPass = VK_NULL_HANDLE;
            VkFramebuffer framebuffer = VK_NULL_HANDLE;  // Optional associated framebuffer
            uint32_t width = 0;
            uint32_t height = 0;
        };

        RHIRenderPassHandle RegisterRenderPass(VkRenderPass renderPass, VkFramebuffer framebuffer = VK_NULL_HANDLE,
                                               uint32_t width = 0, uint32_t height = 0);
        void UnregisterRenderPass(RHIRenderPassHandle handle);
        RenderPassData GetRenderPassData(RHIRenderPassHandle handle) const;
        VkRenderPass GetRenderPass(RHIRenderPassHandle handle) const;
        VkFramebuffer GetFramebuffer(RHIRenderPassHandle handle) const;

        // ========================================================================
        // Buffer Management
        // ========================================================================
        struct BufferData
        {
            VkBuffer buffer = VK_NULL_HANDLE;
            VmaAllocation allocation = VK_NULL_HANDLE;
            uint64_t size = 0;
            bool cpuVisible = false;
        };

        RHIBufferHandle RegisterBuffer(VkBuffer buffer, VmaAllocation allocation, uint64_t size, bool cpuVisible);
        void UnregisterBuffer(RHIBufferHandle handle);
        BufferData GetBufferData(RHIBufferHandle handle) const;
        VkBuffer GetBuffer(RHIBufferHandle handle) const;
        VmaAllocation GetBufferAllocation(RHIBufferHandle handle) const;

        // ========================================================================
        // Texture Management
        // ========================================================================
        struct TextureData
        {
            VkImage image = VK_NULL_HANDLE;
            VkImageView imageView = VK_NULL_HANDLE;
            VkSampler sampler = VK_NULL_HANDLE;
            VmaAllocation allocation = VK_NULL_HANDLE;
            uint32_t width = 0;
            uint32_t height = 0;
            TextureFormat format = TextureFormat::Undefined;
        };

        RHITextureHandle RegisterTexture(VkImage image, VkImageView view, VkSampler sampler,
                                         VmaAllocation allocation, uint32_t width, uint32_t height,
                                         TextureFormat format);
        void UnregisterTexture(RHITextureHandle handle);
        TextureData GetTextureData(RHITextureHandle handle) const;
        VkImage GetTextureImage(RHITextureHandle handle) const;
        VkImageView GetTextureView(RHITextureHandle handle) const;
        VkSampler GetTextureSampler(RHITextureHandle handle) const;

        // ========================================================================
        // Shader Module Management (individual stages)
        // ========================================================================
        struct ShaderModuleData
        {
            VkShaderModule module = VK_NULL_HANDLE;
            ShaderStage stage = ShaderStage::None;
            std::string entryPoint = "main";
        };

        RHIShaderModuleHandle RegisterShaderModule(VkShaderModule module, ShaderStage stage, const std::string& entryPoint = "main");
        void UnregisterShaderModule(RHIShaderModuleHandle handle);
        ShaderModuleData GetShaderModuleData(RHIShaderModuleHandle handle) const;
        VkShaderModule GetShaderModule(RHIShaderModuleHandle handle) const;

        // ========================================================================
        // Shader Program Management (collection of modules)
        // ========================================================================
        struct ShaderData
        {
            std::vector<RHIShaderModuleHandle> moduleHandles;
        };

        RHIShaderHandle RegisterShader(const std::vector<RHIShaderModuleHandle>& moduleHandles);
        void UnregisterShader(RHIShaderHandle handle);
        ShaderData GetShaderData(RHIShaderHandle handle) const;

        // Helper: Get VkPipelineShaderStageCreateInfo array for a shader
        std::vector<VkPipelineShaderStageCreateInfo> GetShaderPipelineStageCreateInfos(RHIShaderHandle handle) const;

        // ========================================================================
        // Descriptor Set Layout Management
        // ========================================================================
        RHIDescriptorSetLayoutHandle RegisterDescriptorSetLayout(VkDescriptorSetLayout layout);
        void UnregisterDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle);
        VkDescriptorSetLayout GetDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle) const;

        // ========================================================================
        // Descriptor Set Management
        // ========================================================================
        struct DescriptorSetData
        {
            VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
            RHIDescriptorSetLayoutHandle layoutHandle;
            VkDescriptorPool owningPool = VK_NULL_HANDLE;  // If non-null, this pool is owned and will be destroyed with the set
        };

        RHIDescriptorSetHandle RegisterDescriptorSet(VkDescriptorSet set, RHIDescriptorSetLayoutHandle layoutHandle,
                                                      VkDescriptorPool owningPool = VK_NULL_HANDLE);
        void UnregisterDescriptorSet(RHIDescriptorSetHandle handle);
        DescriptorSetData GetDescriptorSetData(RHIDescriptorSetHandle handle) const;
        VkDescriptorSet GetDescriptorSet(RHIDescriptorSetHandle handle) const;

        // ========================================================================
        // Command Buffer Management
        // ========================================================================
        // Command buffers are special - they're managed per-frame by VulkanContext
        // We provide a way to get a handle for the current frame's command buffer

        void SetCurrentCommandBuffer(uint32_t frameIndex, VkCommandBuffer cmd);
        RHICommandBufferHandle GetCurrentCommandBufferHandle(uint32_t frameIndex) const;
        VkCommandBuffer GetCommandBuffer(RHICommandBufferHandle handle) const;

        // Immediate command buffer (for one-shot operations, separate from frame buffers)
        void SetImmediateCommandBuffer(VkCommandBuffer cmd);
        RHICommandBufferHandle GetImmediateCommandBufferHandle() const;

        // ========================================================================
        // Cleanup
        // ========================================================================
        void Clear();

    private:
        VulkanResourceRegistry() = default;
        ~VulkanResourceRegistry() = default;

        uint64_t GenerateId();

        mutable std::mutex m_Mutex;
        uint64_t m_NextId = 1;

        // Resource storage
        std::unordered_map<uint64_t, PipelineData> m_Pipelines;
        std::unordered_map<uint64_t, VkPipelineLayout> m_PipelineLayouts;
        std::unordered_map<uint64_t, RenderPassData> m_RenderPasses;
        std::unordered_map<uint64_t, BufferData> m_Buffers;
        std::unordered_map<uint64_t, TextureData> m_Textures;
        std::unordered_map<uint64_t, ShaderModuleData> m_ShaderModules;
        std::unordered_map<uint64_t, ShaderData> m_Shaders;
        std::unordered_map<uint64_t, VkDescriptorSetLayout> m_DescriptorSetLayouts;
        std::unordered_map<uint64_t, DescriptorSetData> m_DescriptorSets;

        // Command buffer tracking (per frame)
        static constexpr uint32_t MaxFramesInFlight = 2;
        VkCommandBuffer m_CommandBuffers[MaxFramesInFlight] = {};
        uint64_t m_CommandBufferHandleIds[MaxFramesInFlight] = {};

        // Immediate command buffer (for one-shot operations)
        static constexpr uint64_t ImmediateCommandBufferHandleId = 0xFFFE0000;
        VkCommandBuffer m_ImmediateCommandBuffer = VK_NULL_HANDLE;
    };

}
