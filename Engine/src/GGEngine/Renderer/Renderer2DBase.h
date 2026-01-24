#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/Renderer/UniformBuffer.h"
#include "GGEngine/Renderer/DescriptorSet.h"
#include "GGEngine/Renderer/Pipeline.h"
#include "GGEngine/Asset/Texture.h"
#include "GGEngine/Renderer/BindlessTextureManager.h"

namespace GGEngine {

    class SceneCamera;

    // Base class for 2D renderers providing shared infrastructure:
    // - Camera UBO management (per-frame uniform buffers and descriptor sets)
    // - White texture for solid color rendering
    // - Pipeline management with render pass tracking
    // - Viewport and scissor state
    // - Common BeginScene logic
    //
    // Derived classes implement their specific vertex/instance buffer management
    // and flush logic.
    class GG_API Renderer2DBase
    {
    protected:
        static constexpr uint32_t MaxFramesInFlight = 2;

        // Camera resources (Set 0)
        Scope<UniformBuffer> m_CameraUniformBuffers[MaxFramesInFlight];
        Scope<DescriptorSetLayout> m_CameraDescriptorLayout;
        Scope<DescriptorSet> m_CameraDescriptorSets[MaxFramesInFlight];

        // White texture for solid colors (bound via bindless)
        Scope<Texture> m_WhiteTexture;
        BindlessTextureIndex m_WhiteTextureIndex = InvalidBindlessIndex;

        // Pipeline
        Scope<Pipeline> m_Pipeline;
        RHIRenderPassHandle m_CurrentRenderPass;

        // Render state
        RHICommandBufferHandle m_CurrentCommandBuffer;
        uint32_t m_CurrentFrameIndex = 0;
        uint32_t m_ViewportWidth = 0;
        uint32_t m_ViewportHeight = 0;
        bool m_SceneStarted = false;

        // Buffer growth flag
        bool m_NeedsBufferGrowth = false;

    public:
        Renderer2DBase() = default;
        virtual ~Renderer2DBase() = default;

        // Shared BeginScene implementation (public for static wrapper access)
        // Returns true if scene was successfully started
        // Calls OnBeginScene() for derived class customization
        bool BeginSceneInternal(const CameraUBO& cameraUBO,
                               RHIRenderPassHandle renderPass,
                               RHICommandBufferHandle cmd,
                               uint32_t viewportWidth,
                               uint32_t viewportHeight);

        // State accessors for static wrapper functions
        bool IsSceneStarted() const { return m_SceneStarted; }
        void SetSceneStarted(bool started) { m_SceneStarted = started; }
        void ClearCommandBuffer() { m_CurrentCommandBuffer = RHICommandBufferHandle{}; }
        BindlessTextureIndex GetWhiteTextureIndex() const { return m_WhiteTextureIndex; }
        bool NeedsBufferGrowth() const { return m_NeedsBufferGrowth; }
        void RequestBufferGrowth() { m_NeedsBufferGrowth = true; }

    protected:
        // Initialize shared resources (camera UBO, descriptors, white texture)
        void InitBase();

        // Shutdown shared resources
        void ShutdownBase();

        // Set viewport and scissor from stored dimensions
        void SetViewportAndScissor();

        // Bind camera descriptor set (Set 0)
        void BindCameraDescriptorSet(RHIPipelineLayoutHandle pipelineLayout);

        // Bind bindless texture descriptor set (Set 1)
        void BindBindlessDescriptorSet(RHIPipelineLayoutHandle pipelineLayout);

        // Pure virtual: Called at start of BeginScene after shared setup
        virtual void OnBeginScene() = 0;

        // Pure virtual: Called when render pass changes and pipeline needs recreation
        virtual void RecreatePipeline(RHIRenderPassHandle renderPass) = 0;

        // Pure virtual: Called when buffer growth is needed
        virtual void GrowBuffers() = 0;

        // Pure virtual: Get current capacity for growth checks
        virtual uint32_t GetCurrentCapacity() const = 0;
        virtual uint32_t GetAbsoluteMaxCapacity() const = 0;
    };

}
