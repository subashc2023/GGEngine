#pragma once

#include "RHITypes.h"
#include "RHIEnums.h"
#include "RHISpecifications.h"
#include "GGEngine/Core/Core.h"
#include "GGEngine/Core/Result.h"

#include <string>
#include <vector>
#include <functional>

namespace GGEngine {

    // Forward declarations
    class VertexLayout;

    // ============================================================================
    // Descriptor Binding (backend-agnostic)
    // ============================================================================
    struct GG_API RHIDescriptorBinding
    {
        uint32_t binding = 0;
        DescriptorType type = DescriptorType::UniformBuffer;
        ShaderStage stages = ShaderStage::AllGraphics;
        uint32_t count = 1;
    };

    // ============================================================================
    // Sampler Specification (backend-agnostic)
    // ============================================================================
    struct GG_API RHISamplerSpecification
    {
        Filter minFilter = Filter::Linear;
        Filter magFilter = Filter::Linear;
        MipmapMode mipmapMode = MipmapMode::Linear;
        AddressMode addressModeU = AddressMode::Repeat;
        AddressMode addressModeV = AddressMode::Repeat;
        AddressMode addressModeW = AddressMode::Repeat;
        float mipLodBias = 0.0f;
        bool anisotropyEnable = false;
        float maxAnisotropy = 1.0f;
        bool compareEnable = false;
        CompareOp compareOp = CompareOp::Always;
        float minLod = 0.0f;
        float maxLod = 1000.0f;
        BorderColor borderColor = BorderColor::FloatTransparentBlack;
    };

    // ============================================================================
    // RHI Device
    // ============================================================================
    // The main interface to the graphics backend. Wraps VulkanContext and provides
    // a backend-agnostic interface for resource creation and frame management.
    //
    // This is a singleton that mirrors VulkanContext::Get() for easy migration.
    // ============================================================================

    class GG_API RHIDevice
    {
    public:
        static RHIDevice& Get();

        // ========================================================================
        // Lifecycle
        // ========================================================================

        // Initialize the device with a native window handle
        void Init(void* windowHandle);

        // Shutdown and release all resources
        void Shutdown();

        // ========================================================================
        // Frame Management
        // ========================================================================

        // Begin a new frame (waits for previous frame's fence, acquires swapchain image)
        void BeginFrame();

        // End the frame (submits command buffer, presents swapchain image)
        void EndFrame();

        // Begin the swapchain render pass (for ImGui and direct swapchain rendering)
        void BeginSwapchainRenderPass();

        // ========================================================================
        // Frame State
        // ========================================================================

        // Get the command buffer for the current frame
        RHICommandBufferHandle GetCurrentCommandBuffer() const;

        // Get the swapchain render pass handle
        RHIRenderPassHandle GetSwapchainRenderPass() const;

        // Get swapchain dimensions
        uint32_t GetSwapchainWidth() const;
        uint32_t GetSwapchainHeight() const;

        // Get current frame index (0 or 1 for double-buffering)
        uint32_t GetCurrentFrameIndex() const;

        // Maximum frames in flight
        static constexpr uint32_t GetMaxFramesInFlight() { return 2; }

        // ========================================================================
        // Synchronization
        // ========================================================================

        // Execute a command function immediately and wait for completion
        void ImmediateSubmit(const std::function<void(RHICommandBufferHandle)>& func);

        // Wait for device to be idle (all queues finished)
        void WaitIdle();

        // ========================================================================
        // Window/Display
        // ========================================================================

        // Handle window resize
        void OnWindowResize(uint32_t width, uint32_t height);

        // VSync control
        void SetVSync(bool enabled);
        bool IsVSync() const;

        // ========================================================================
        // Resource Creation (Convenience - delegates to VulkanContext internally)
        // ========================================================================

        // These methods provide a cleaner API for resource creation.
        // They internally use VulkanContext and register resources with VulkanResourceRegistry.

        // ========================================================================
        // Descriptor Set Management
        // ========================================================================

        // Create a descriptor set layout
        RHIDescriptorSetLayoutHandle CreateDescriptorSetLayout(const std::vector<RHIDescriptorBinding>& bindings);
        void DestroyDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle);

        // Allocate a descriptor set from the global pool
        RHIDescriptorSetHandle AllocateDescriptorSet(RHIDescriptorSetLayoutHandle layout);
        void FreeDescriptorSet(RHIDescriptorSetHandle handle);

        // Update descriptor set bindings
        void UpdateDescriptorSet(RHIDescriptorSetHandle set, const std::vector<RHIDescriptorWrite>& writes);

        // ========================================================================
        // Buffer Management
        // ========================================================================

        // Create a GPU buffer (returns NullBuffer on failure, logs error)
        RHIBufferHandle CreateBuffer(const RHIBufferSpecification& spec);

        // Create a GPU buffer with detailed error information
        Result<RHIBufferHandle> TryCreateBuffer(const RHIBufferSpecification& spec);

        void DestroyBuffer(RHIBufferHandle handle);

        // Buffer memory mapping (for CPU-visible buffers)
        void* MapBuffer(RHIBufferHandle handle);
        void UnmapBuffer(RHIBufferHandle handle);
        void FlushBuffer(RHIBufferHandle handle, uint64_t offset = 0, uint64_t size = 0);

        // Upload data to buffer (uses staging if not CPU-visible)
        void UploadBufferData(RHIBufferHandle handle, const void* data, uint64_t size, uint64_t offset = 0);

        // ========================================================================
        // Texture Management
        // ========================================================================

        // Create a texture/image (returns NullTexture on failure, logs error)
        RHITextureHandle CreateTexture(const RHITextureSpecification& spec);

        // Create a texture/image with detailed error information
        Result<RHITextureHandle> TryCreateTexture(const RHITextureSpecification& spec);

        void DestroyTexture(RHITextureHandle handle);

        // Upload pixel data to texture (creates staging buffer internally)
        void UploadTextureData(RHITextureHandle handle, const void* pixels, uint64_t size);

        // Get texture dimensions
        uint32_t GetTextureWidth(RHITextureHandle handle) const;
        uint32_t GetTextureHeight(RHITextureHandle handle) const;

        // ========================================================================
        // Sampler Management
        // ========================================================================

        RHISamplerHandle CreateSampler(const RHISamplerSpecification& spec);
        void DestroySampler(RHISamplerHandle handle);

        // ========================================================================
        // Shader Management
        // ========================================================================

        // Create shader module from SPIR-V bytecode (returns NullShaderModule on failure, logs error)
        RHIShaderModuleHandle CreateShaderModule(ShaderStage stage, const std::vector<char>& spirvCode);
        RHIShaderModuleHandle CreateShaderModule(ShaderStage stage, const void* spirvData, size_t spirvSize);

        // Create shader module with detailed error information
        Result<RHIShaderModuleHandle> TryCreateShaderModule(ShaderStage stage, const std::vector<char>& spirvCode);
        Result<RHIShaderModuleHandle> TryCreateShaderModule(ShaderStage stage, const void* spirvData, size_t spirvSize);

        void DestroyShaderModule(RHIShaderModuleHandle handle);

        // ========================================================================
        // Pipeline Management
        // ========================================================================

        // Create graphics pipeline (returns invalid result on failure, logs error)
        RHIGraphicsPipelineResult CreateGraphicsPipeline(const RHIGraphicsPipelineSpecification& spec);

        // Create graphics pipeline with detailed error information
        Result<RHIGraphicsPipelineResult> TryCreateGraphicsPipeline(const RHIGraphicsPipelineSpecification& spec);

        void DestroyPipeline(RHIPipelineHandle handle);
        void DestroyPipelineLayout(RHIPipelineLayoutHandle handle);

        // Get pipeline layout from pipeline (for push constants/descriptor binding)
        RHIPipelineLayoutHandle GetPipelineLayout(RHIPipelineHandle pipeline) const;

        // ========================================================================
        // Render Pass Management
        // ========================================================================

        RHIRenderPassHandle CreateRenderPass(const RHIRenderPassSpecification& spec);
        void DestroyRenderPass(RHIRenderPassHandle handle);

        // ========================================================================
        // Framebuffer Management
        // ========================================================================

        RHIFramebufferHandle CreateFramebuffer(const RHIFramebufferSpecification& spec);
        void DestroyFramebuffer(RHIFramebufferHandle handle);

        // ========================================================================
        // Bindless Texture Support
        // ========================================================================

        // Query maximum supported bindless textures
        uint32_t GetMaxBindlessTextures() const;

        // Create descriptor set layout for bindless texture array
        RHIDescriptorSetLayoutHandle CreateBindlessTextureLayout(uint32_t maxTextures);

        // Allocate bindless descriptor set with variable count
        RHIDescriptorSetHandle AllocateBindlessDescriptorSet(RHIDescriptorSetLayoutHandle layout, uint32_t maxTextures);

        // Update a single texture slot in bindless array
        void UpdateBindlessTexture(RHIDescriptorSetHandle set, uint32_t index,
                                   RHITextureHandle texture, RHISamplerHandle sampler);

        // Get raw descriptor set handle for binding (needed for RHICmd::BindDescriptorSetRaw)
        void* GetRawDescriptorSet(RHIDescriptorSetHandle handle) const;

        // ========================================================================
        // Bindless Texture Support (Separate Sampler Pattern)
        // ========================================================================
        // For MoltenVK/Metal compatibility, uses separate sampler (binding 0) and
        // texture array (binding 1) instead of combined image samplers.

        // Create layout with: binding 0 = immutable sampler, binding 1 = texture array
        RHIDescriptorSetLayoutHandle CreateBindlessSamplerTextureLayout(
            RHISamplerHandle immutableSampler, uint32_t maxTextures);

        // Allocate descriptor set with variable texture count
        RHIDescriptorSetHandle AllocateBindlessSamplerTextureSet(
            RHIDescriptorSetLayoutHandle layout, uint32_t maxTextures);

        // Update a single texture slot (binding 1, SAMPLED_IMAGE type)
        void UpdateBindlessSamplerTextureSlot(
            RHIDescriptorSetHandle set, uint32_t index, RHITextureHandle texture);

        // ========================================================================
        // ImGui Integration
        // ========================================================================

        // Register a texture for use with ImGui::Image() (returns opaque handle)
        void* RegisterImGuiTexture(RHITextureHandle texture, RHISamplerHandle sampler);

        // Unregister a texture from ImGui
        void UnregisterImGuiTexture(void* imguiHandle);

    private:
        RHIDevice() = default;
        ~RHIDevice() = default;
        RHIDevice(const RHIDevice&) = delete;
        RHIDevice& operator=(const RHIDevice&) = delete;

        // Cached handles for frequently accessed resources
        RHIRenderPassHandle m_SwapchainRenderPassHandle;
        bool m_Initialized = false;
    };

}
