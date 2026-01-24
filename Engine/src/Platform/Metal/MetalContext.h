#pragma once

#include "GGEngine/Core/Core.h"

#ifdef __OBJC__
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>
#else
// Forward declarations for C++ headers
typedef void* MTLDevicePtr;
typedef void* MTLCommandQueuePtr;
typedef void* MTLCommandBufferPtr;
typedef void* MTLRenderCommandEncoderPtr;
typedef void* CAMetalLayerPtr;
typedef void* CAMetalDrawablePtr;
typedef void* dispatch_semaphore_t_ptr;
#endif

#include <functional>
#include <cstdint>

struct GLFWwindow;

namespace GGEngine {

    class GG_API MetalContext
    {
    public:
        static MetalContext& Get();

        void Init(GLFWwindow* window);
        void Shutdown();

        void BeginFrame();
        void BeginSwapchainRenderPass();
        void EndFrame();

        void OnWindowResize(uint32_t width, uint32_t height);

        void SetVSync(bool enabled);
        bool IsVSync() const { return m_VSync; }

#ifdef __OBJC__
        id<MTLDevice> GetDevice() const { return m_Device; }
        id<MTLCommandQueue> GetCommandQueue() const { return m_CommandQueue; }
        id<MTLCommandBuffer> GetCurrentCommandBuffer() const { return m_CommandBuffers[m_CurrentFrameIndex]; }
        id<MTLRenderCommandEncoder> GetCurrentRenderEncoder() const { return m_CurrentRenderEncoder; }
        CAMetalLayer* GetMetalLayer() const { return m_MetalLayer; }
        id<CAMetalDrawable> GetCurrentDrawable() const { return m_CurrentDrawable; }
#else
        MTLDevicePtr GetDevice() const { return m_Device; }
        MTLCommandQueuePtr GetCommandQueue() const { return m_CommandQueue; }
        MTLCommandBufferPtr GetCurrentCommandBuffer() const { return m_CommandBuffers[m_CurrentFrameIndex]; }
        MTLRenderCommandEncoderPtr GetCurrentRenderEncoder() const { return m_CurrentRenderEncoder; }
        CAMetalLayerPtr GetMetalLayer() const { return m_MetalLayer; }
        CAMetalDrawablePtr GetCurrentDrawable() const { return m_CurrentDrawable; }
#endif

        uint32_t GetSwapchainWidth() const { return m_DrawableWidth; }
        uint32_t GetSwapchainHeight() const { return m_DrawableHeight; }
        uint32_t GetCurrentFrameIndex() const { return m_CurrentFrameIndex; }
        static constexpr uint32_t GetMaxFramesInFlight() { return MAX_FRAMES_IN_FLIGHT; }

        // Bindless rendering limits (Metal 3 argument buffers)
        struct BindlessLimits {
            uint32_t maxSampledImages = 500000;  // Metal 3 supports massive arrays
            uint32_t maxPerStageDescriptorSampledImages = 500000;
            uint32_t maxSamplers = 2048;
            uint32_t maxPerStageDescriptorSamplers = 2048;
        };
        const BindlessLimits& GetBindlessLimits() const { return m_BindlessLimits; }

        // Execute a one-time command buffer synchronously (blocks until complete)
#ifdef __OBJC__
        void ImmediateSubmit(const std::function<void(id<MTLCommandBuffer>)>& func);
#endif

        // Wait for all GPU work to complete
        void WaitIdle();

    private:
        MetalContext() = default;
        ~MetalContext() = default;

        void CreateDevice();
        void CreateCommandQueue();
        void SetupMetalLayer();
        void CreateSyncObjects();
        void QueryBindlessLimits();

    private:
        GLFWwindow* m_Window = nullptr;

#ifdef __OBJC__
        id<MTLDevice> m_Device = nil;
        id<MTLCommandQueue> m_CommandQueue = nil;
        CAMetalLayer* m_MetalLayer = nullptr;

        // Double-buffered command buffers
        id<MTLCommandBuffer> m_CommandBuffers[MAX_FRAMES_IN_FLIGHT];
        id<MTLRenderCommandEncoder> m_CurrentRenderEncoder = nil;
        id<CAMetalDrawable> m_CurrentDrawable = nil;

        // Frame synchronization
        dispatch_semaphore_t m_FrameSemaphores[MAX_FRAMES_IN_FLIGHT];
#else
        MTLDevicePtr m_Device = nullptr;
        MTLCommandQueuePtr m_CommandQueue = nullptr;
        CAMetalLayerPtr m_MetalLayer = nullptr;
        MTLCommandBufferPtr m_CommandBuffers[MAX_FRAMES_IN_FLIGHT];
        MTLRenderCommandEncoderPtr m_CurrentRenderEncoder = nullptr;
        CAMetalDrawablePtr m_CurrentDrawable = nullptr;
        dispatch_semaphore_t_ptr m_FrameSemaphores[MAX_FRAMES_IN_FLIGHT];
#endif

        BindlessLimits m_BindlessLimits;

        uint32_t m_DrawableWidth = 0;
        uint32_t m_DrawableHeight = 0;
        uint32_t m_CurrentFrameIndex = 0;
        bool m_FrameStarted = false;
        bool m_VSync = true;
        bool m_Initialized = false;

        static constexpr int MAX_FRAMES_IN_FLIGHT = 2;
    };

}
