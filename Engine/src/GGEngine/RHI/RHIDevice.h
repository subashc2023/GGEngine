#pragma once

#include "RHITypes.h"
#include "RHIEnums.h"
#include "GGEngine/Core/Core.h"

#include <string>
#include <vector>
#include <functional>

namespace GGEngine {

    // Forward declarations
    class VertexLayout;

    // ============================================================================
    // Push Constant Range (backend-agnostic)
    // ============================================================================
    struct GG_API RHIPushConstantRange
    {
        ShaderStage stages = ShaderStage::AllGraphics;
        uint32_t offset = 0;
        uint32_t size = 0;
    };

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

        // Create a descriptor set layout
        RHIDescriptorSetLayoutHandle CreateDescriptorSetLayout(const std::vector<RHIDescriptorBinding>& bindings);
        void DestroyDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle);

        // Allocate a descriptor set from the global pool
        RHIDescriptorSetHandle AllocateDescriptorSet(RHIDescriptorSetLayoutHandle layout);
        void FreeDescriptorSet(RHIDescriptorSetHandle handle);

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
