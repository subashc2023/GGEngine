#include "ggpch.h"
#include "MetalContext.h"
#include "MetalRHI.h"
#include "MetalUtils.h"
#include "GGEngine/Core/Log.h"

#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>
#import <Cocoa/Cocoa.h>

#define GLFW_INCLUDE_NONE
#define GLFW_EXPOSE_NATIVE_COCOA
#include <GLFW/glfw3.h>
#include <GLFW/glfw3native.h>

namespace GGEngine {

    MetalContext& MetalContext::Get()
    {
        static MetalContext instance;
        return instance;
    }

    void MetalContext::Init(GLFWwindow* window)
    {
        GG_CORE_INFO("MetalContext: Initializing...");

        m_Window = window;

        CreateDevice();
        CreateCommandQueue();
        SetupMetalLayer();
        CreateSyncObjects();
        QueryBindlessLimits();

        m_Initialized = true;

        GG_CORE_INFO("MetalContext: Initialized successfully");
        GG_CORE_INFO("  Device: {}", [[m_Device name] UTF8String]);
        GG_CORE_INFO("  Drawable size: {}x{}", m_DrawableWidth, m_DrawableHeight);
        GG_CORE_INFO("  Max frames in flight: {}", MAX_FRAMES_IN_FLIGHT);
    }

    void MetalContext::Shutdown()
    {
        if (!m_Initialized)
            return;

        GG_CORE_INFO("MetalContext: Shutting down...");

        WaitIdle();

        // Release sync objects
        for (int i = 0; i < MAX_FRAMES_IN_FLIGHT; i++)
        {
            if (m_FrameSemaphores[i])
            {
                // dispatch_release is handled by ARC
                m_FrameSemaphores[i] = nil;
            }
            m_CommandBuffers[i] = nil;
        }

        m_CurrentRenderEncoder = nil;
        m_CurrentDrawable = nil;
        m_MetalLayer = nullptr;
        m_CommandQueue = nil;
        m_Device = nil;
        m_Window = nullptr;
        m_Initialized = false;

        GG_CORE_INFO("MetalContext: Shutdown complete");
    }

    void MetalContext::CreateDevice()
    {
        m_Device = MTLCreateSystemDefaultDevice();
        MTL_CHECK_RETURN(m_Device != nil, "Failed to create Metal device");

        GG_CORE_TRACE("MetalContext: Created Metal device");
    }

    void MetalContext::CreateCommandQueue()
    {
        m_CommandQueue = [m_Device newCommandQueue];
        MTL_CHECK_RETURN(m_CommandQueue != nil, "Failed to create command queue");

        GG_CORE_TRACE("MetalContext: Created command queue");
    }

    void MetalContext::SetupMetalLayer()
    {
        // Get the NSWindow from GLFW
        NSWindow* nsWindow = glfwGetCocoaWindow(m_Window);
        MTL_CHECK_RETURN(nsWindow != nil, "Failed to get NSWindow from GLFW");

        // Create and configure CAMetalLayer
        m_MetalLayer = [CAMetalLayer layer];
        m_MetalLayer.device = m_Device;
        m_MetalLayer.pixelFormat = MTLPixelFormatBGRA8Unorm;
        m_MetalLayer.framebufferOnly = YES;
        m_MetalLayer.displaySyncEnabled = m_VSync;

        // Get content scale for Retina displays
        int fbWidth, fbHeight;
        glfwGetFramebufferSize(m_Window, &fbWidth, &fbHeight);
        m_MetalLayer.drawableSize = CGSizeMake(fbWidth, fbHeight);
        m_MetalLayer.contentsScale = nsWindow.backingScaleFactor;

        m_DrawableWidth = static_cast<uint32_t>(fbWidth);
        m_DrawableHeight = static_cast<uint32_t>(fbHeight);

        // Set the layer on the window's content view
        nsWindow.contentView.layer = m_MetalLayer;
        nsWindow.contentView.wantsLayer = YES;

        GG_CORE_TRACE("MetalContext: Setup CAMetalLayer ({}x{}, scale={})",
                      fbWidth, fbHeight, m_MetalLayer.contentsScale);
    }

    void MetalContext::CreateSyncObjects()
    {
        // Create semaphores for frame pacing (double buffering)
        for (int i = 0; i < MAX_FRAMES_IN_FLIGHT; i++)
        {
            m_FrameSemaphores[i] = dispatch_semaphore_create(1);
            MTL_CHECK_RETURN(m_FrameSemaphores[i] != nil, "Failed to create frame semaphore");
        }

        GG_CORE_TRACE("MetalContext: Created {} frame semaphores", MAX_FRAMES_IN_FLIGHT);
    }

    void MetalContext::QueryBindlessLimits()
    {
        // Metal 3 on Apple Silicon supports massive argument buffer sizes
        // Query actual device limits
        if (@available(macOS 13.0, *))
        {
            // Metal 3 supports very large argument buffers
            m_BindlessLimits.maxSampledImages = 500000;
            m_BindlessLimits.maxPerStageDescriptorSampledImages = 500000;
            m_BindlessLimits.maxSamplers = 2048;
            m_BindlessLimits.maxPerStageDescriptorSamplers = 2048;
        }
        else
        {
            // Older macOS - more conservative limits
            m_BindlessLimits.maxSampledImages = 128;
            m_BindlessLimits.maxPerStageDescriptorSampledImages = 128;
            m_BindlessLimits.maxSamplers = 16;
            m_BindlessLimits.maxPerStageDescriptorSamplers = 16;
        }

        GG_CORE_INFO("MetalContext: Bindless limits - maxTextures={}, maxSamplers={}",
                     m_BindlessLimits.maxSampledImages, m_BindlessLimits.maxSamplers);
    }

    void MetalContext::BeginFrame()
    {
        if (m_FrameStarted)
        {
            GG_CORE_WARN("MetalContext::BeginFrame called while frame already in progress");
            return;
        }

        // Wait for this frame's previous work to complete (frame pacing)
        dispatch_semaphore_wait(m_FrameSemaphores[m_CurrentFrameIndex], DISPATCH_TIME_FOREVER);

        // Get next drawable
        m_CurrentDrawable = [m_MetalLayer nextDrawable];
        if (!m_CurrentDrawable)
        {
            GG_CORE_ERROR("MetalContext: Failed to get next drawable");
            dispatch_semaphore_signal(m_FrameSemaphores[m_CurrentFrameIndex]);
            return;
        }

        // Create command buffer for this frame
        m_CommandBuffers[m_CurrentFrameIndex] = [m_CommandQueue commandBuffer];
        if (!m_CommandBuffers[m_CurrentFrameIndex])
        {
            GG_CORE_ERROR("MetalContext: Failed to create command buffer");
            dispatch_semaphore_signal(m_FrameSemaphores[m_CurrentFrameIndex]);
            return;
        }

        // Register command buffer with resource registry
        MetalResourceRegistry::Get().SetCurrentCommandBuffer(m_CurrentFrameIndex,
                                                              m_CommandBuffers[m_CurrentFrameIndex]);

        m_FrameStarted = true;
    }

    void MetalContext::BeginSwapchainRenderPass()
    {
        if (!m_FrameStarted || !m_CurrentDrawable)
        {
            GG_CORE_ERROR("MetalContext::BeginSwapchainRenderPass called without valid frame");
            return;
        }

        // End any existing render encoder
        if (m_CurrentRenderEncoder)
        {
            [m_CurrentRenderEncoder endEncoding];
            m_CurrentRenderEncoder = nil;
        }

        // Create render pass descriptor for swapchain
        MTLRenderPassDescriptor* renderPassDescriptor = [MTLRenderPassDescriptor renderPassDescriptor];
        renderPassDescriptor.colorAttachments[0].texture = m_CurrentDrawable.texture;
        renderPassDescriptor.colorAttachments[0].loadAction = MTLLoadActionClear;
        renderPassDescriptor.colorAttachments[0].storeAction = MTLStoreActionStore;
        renderPassDescriptor.colorAttachments[0].clearColor = MTLClearColorMake(0.1, 0.1, 0.1, 1.0);

        // Create render command encoder
        m_CurrentRenderEncoder = [m_CommandBuffers[m_CurrentFrameIndex]
                                  renderCommandEncoderWithDescriptor:renderPassDescriptor];

        if (!m_CurrentRenderEncoder)
        {
            GG_CORE_ERROR("MetalContext: Failed to create render command encoder");
            return;
        }

        [m_CurrentRenderEncoder setLabel:@"Swapchain Render Pass"];
    }

    void MetalContext::EndFrame()
    {
        if (!m_FrameStarted)
        {
            GG_CORE_WARN("MetalContext::EndFrame called without active frame");
            return;
        }

        // End any active render encoder
        if (m_CurrentRenderEncoder)
        {
            [m_CurrentRenderEncoder endEncoding];
            m_CurrentRenderEncoder = nil;
        }

        // Schedule drawable presentation
        if (m_CurrentDrawable)
        {
            [m_CommandBuffers[m_CurrentFrameIndex] presentDrawable:m_CurrentDrawable];
        }

        // Add completion handler to signal semaphore when GPU finishes
        __block dispatch_semaphore_t semaphore = m_FrameSemaphores[m_CurrentFrameIndex];
        [m_CommandBuffers[m_CurrentFrameIndex] addCompletedHandler:^(id<MTLCommandBuffer> _Nonnull buffer) {
            dispatch_semaphore_signal(semaphore);
        }];

        // Submit command buffer
        [m_CommandBuffers[m_CurrentFrameIndex] commit];

        // Advance to next frame
        m_CurrentFrameIndex = (m_CurrentFrameIndex + 1) % MAX_FRAMES_IN_FLIGHT;
        m_CurrentDrawable = nil;
        m_FrameStarted = false;
    }

    void MetalContext::OnWindowResize(uint32_t width, uint32_t height)
    {
        if (width == 0 || height == 0)
            return;

        // Wait for GPU to finish before resizing
        WaitIdle();

        // Update drawable size
        m_MetalLayer.drawableSize = CGSizeMake(width, height);
        m_DrawableWidth = width;
        m_DrawableHeight = height;

        GG_CORE_TRACE("MetalContext: Resized to {}x{}", width, height);
    }

    void MetalContext::SetVSync(bool enabled)
    {
        m_VSync = enabled;
        if (m_MetalLayer)
        {
            m_MetalLayer.displaySyncEnabled = enabled;
        }
    }

    void MetalContext::ImmediateSubmit(const std::function<void(id<MTLCommandBuffer>)>& func)
    {
        @autoreleasepool
        {
            id<MTLCommandBuffer> cmd = [m_CommandQueue commandBuffer];
            MTL_CHECK_RETURN(cmd != nil, "Failed to create immediate command buffer");

            // Register for immediate use
            MetalResourceRegistry::Get().SetImmediateCommandBuffer(cmd);

            // Execute user function
            func(cmd);

            // Submit and wait
            [cmd commit];
            [cmd waitUntilCompleted];

            // Check for errors
            if (cmd.error)
            {
                GG_CORE_ERROR("MetalContext::ImmediateSubmit error: {}",
                              [[cmd.error localizedDescription] UTF8String]);
            }

            MetalResourceRegistry::Get().SetImmediateCommandBuffer(nil);
        }
    }

    void MetalContext::WaitIdle()
    {
        // Wait for all frames to complete
        for (int i = 0; i < MAX_FRAMES_IN_FLIGHT; i++)
        {
            dispatch_semaphore_wait(m_FrameSemaphores[i], DISPATCH_TIME_FOREVER);
            dispatch_semaphore_signal(m_FrameSemaphores[i]);
        }
    }

}
