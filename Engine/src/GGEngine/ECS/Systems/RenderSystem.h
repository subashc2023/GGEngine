#pragma once

#include "GGEngine/ECS/System.h"
#include "GGEngine/RHI/RHITypes.h"
#include <glm/glm.hpp>

namespace GGEngine {

    class Camera;
    class SceneCamera;

    // =============================================================================
    // RenderContext
    // =============================================================================
    // Contains all state needed by render systems to perform rendering.
    // Set by the application layer before executing render systems.
    //
    struct GG_API RenderContext
    {
        // RHI state
        RHIRenderPassHandle RenderPass;
        RHICommandBufferHandle CommandBuffer;
        uint32_t ViewportWidth = 0;
        uint32_t ViewportHeight = 0;

        // Camera options (one of these should be set)
        // Option 1: External camera (editor, debug views)
        const Camera* ExternalCamera = nullptr;

        // Option 2: Scene camera with transform (runtime, ECS camera entity)
        const SceneCamera* RuntimeCamera = nullptr;
        const glm::mat4* CameraTransform = nullptr;

        // Helper to check if context is valid for rendering
        bool IsValid() const
        {
            bool hasCamera = (ExternalCamera != nullptr) ||
                             (RuntimeCamera != nullptr && CameraTransform != nullptr);
            return RenderPass.IsValid() && CommandBuffer.IsValid() &&
                   ViewportWidth > 0 && ViewportHeight > 0 && hasCamera;
        }

        bool UsesRuntimeCamera() const
        {
            return RuntimeCamera != nullptr && CameraTransform != nullptr;
        }
    };

    // =============================================================================
    // IRenderSystem
    // =============================================================================
    // Base class for all render systems. Extends ISystem with render-specific
    // functionality like RenderContext management.
    //
    // Render systems are responsible for:
    // 1. Calling Renderer2D::BeginScene() or InstancedRenderer2D::BeginScene()
    // 2. Iterating components and issuing draw calls
    // 3. Calling EndScene()
    //
    // Example usage:
    //   // In your layer's render method:
    //   RenderContext ctx;
    //   ctx.RenderPass = framebuffer->GetRenderPass();
    //   ctx.CommandBuffer = device.GetCurrentCommandBuffer();
    //   ctx.ViewportWidth = width;
    //   ctx.ViewportHeight = height;
    //   ctx.ExternalCamera = &editorCamera;
    //
    //   spriteSystem->SetRenderContext(ctx);
    //   scheduler.Execute(scene, deltaTime);
    //
    class GG_API IRenderSystem : public ISystem
    {
    public:
        virtual ~IRenderSystem() = default;

        // Set the render context before Execute() is called
        void SetRenderContext(const RenderContext& context) { m_RenderContext = context; }
        const RenderContext& GetRenderContext() const { return m_RenderContext; }

    protected:
        RenderContext m_RenderContext;
    };

}
