#pragma once

#include "RHITypes.h"
#include "RHIEnums.h"
#include "GGEngine/Core/Core.h"

#include <cstdint>

namespace GGEngine {

    // ============================================================================
    // RHI Command Buffer API
    // ============================================================================
    // Static API for recording GPU commands. Mirrors the RenderCommand pattern
    // but uses RHI handle types instead of Vulkan types.
    //
    // All methods are static for ease of use and zero overhead.
    // The backend implementation translates handles to actual GPU commands.
    // ============================================================================

    class GG_API RHICmd
    {
    public:
        // ========================================================================
        // Viewport & Scissor
        // ========================================================================

        // Set viewport (full parameters)
        static void SetViewport(RHICommandBufferHandle cmd, float x, float y, float width, float height,
                                float minDepth = 0.0f, float maxDepth = 1.0f);

        // Set viewport (0,0 origin, float dimensions)
        static void SetViewport(RHICommandBufferHandle cmd, float width, float height);

        // Set viewport (0,0 origin, uint dimensions)
        static void SetViewport(RHICommandBufferHandle cmd, uint32_t width, uint32_t height);

        // Set scissor (full parameters)
        static void SetScissor(RHICommandBufferHandle cmd, int32_t x, int32_t y, uint32_t width, uint32_t height);

        // Set scissor (0,0 origin)
        static void SetScissor(RHICommandBufferHandle cmd, uint32_t width, uint32_t height);

        // ========================================================================
        // Pipeline Binding
        // ========================================================================

        // Bind a graphics pipeline
        static void BindPipeline(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline);

        // ========================================================================
        // Buffer Binding
        // ========================================================================

        // Bind a vertex buffer
        static void BindVertexBuffer(RHICommandBufferHandle cmd, RHIBufferHandle buffer, uint32_t binding = 0);

        // Bind an index buffer
        static void BindIndexBuffer(RHICommandBufferHandle cmd, RHIBufferHandle buffer, IndexType indexType);

        // ========================================================================
        // Descriptor Set Binding
        // ========================================================================

        // Bind a descriptor set to the pipeline
        static void BindDescriptorSet(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                                      RHIDescriptorSetHandle set, uint32_t setIndex = 0);

        // ========================================================================
        // Push Constants
        // ========================================================================

        // Push constant data
        static void PushConstants(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                                  ShaderStage stages, uint32_t offset, uint32_t size, const void* data);

        // Push constant data (templated)
        template<typename T>
        static void PushConstants(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                                  ShaderStage stages, const T& data, uint32_t offset = 0)
        {
            PushConstants(cmd, pipeline, stages, offset, sizeof(T), &data);
        }

        // ========================================================================
        // Draw Commands
        // ========================================================================

        // Draw non-indexed primitives
        static void Draw(RHICommandBufferHandle cmd, uint32_t vertexCount, uint32_t instanceCount = 1,
                         uint32_t firstVertex = 0, uint32_t firstInstance = 0);

        // Draw indexed primitives
        static void DrawIndexed(RHICommandBufferHandle cmd, uint32_t indexCount, uint32_t instanceCount = 1,
                                uint32_t firstIndex = 0, int32_t vertexOffset = 0, uint32_t firstInstance = 0);

        // ========================================================================
        // Render Pass Commands
        // ========================================================================

        // Begin a render pass (for custom framebuffers)
        static void BeginRenderPass(RHICommandBufferHandle cmd, RHIRenderPassHandle renderPass,
                                    RHIFramebufferHandle framebuffer, uint32_t width, uint32_t height,
                                    float clearR = 0.0f, float clearG = 0.0f, float clearB = 0.0f, float clearA = 1.0f);

        // End the current render pass
        static void EndRenderPass(RHICommandBufferHandle cmd);
    };

}
