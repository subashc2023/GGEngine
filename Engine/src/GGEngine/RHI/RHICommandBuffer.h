#pragma once

#include "RHITypes.h"
#include "RHIEnums.h"
#include "RHISpecifications.h"
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

        // Push constant data (using pipeline handle to get layout)
        static void PushConstants(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                                  ShaderStage stages, uint32_t offset, uint32_t size, const void* data);

        // Push constant data (using explicit layout handle)
        static void PushConstants(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
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

        // ========================================================================
        // Transfer Commands
        // ========================================================================

        // Copy data between buffers
        static void CopyBuffer(RHICommandBufferHandle cmd, RHIBufferHandle src, RHIBufferHandle dst,
                               uint64_t srcOffset, uint64_t dstOffset, uint64_t size);

        // Copy buffer data to texture
        static void CopyBufferToTexture(RHICommandBufferHandle cmd, RHIBufferHandle buffer,
                                        RHITextureHandle texture, const RHIBufferImageCopy& region);

        // Simplified copy for 2D textures (copies entire buffer to mip 0, layer 0)
        static void CopyBufferToTexture(RHICommandBufferHandle cmd, RHIBufferHandle buffer,
                                        RHITextureHandle texture, uint32_t width, uint32_t height);

        // ========================================================================
        // Image Layout Transitions
        // ========================================================================

        // Transition image layout (inserts appropriate pipeline barrier)
        static void TransitionImageLayout(RHICommandBufferHandle cmd, RHITextureHandle texture,
                                          ImageLayout oldLayout, ImageLayout newLayout);

        // Transition with explicit mip/layer range
        static void TransitionImageLayout(RHICommandBufferHandle cmd, RHITextureHandle texture,
                                          ImageLayout oldLayout, ImageLayout newLayout,
                                          uint32_t baseMipLevel, uint32_t mipCount,
                                          uint32_t baseArrayLayer, uint32_t layerCount);

        // ========================================================================
        // Pipeline Barriers (Advanced)
        // ========================================================================

        // Execute a pipeline barrier with multiple image transitions
        static void PipelineBarrier(RHICommandBufferHandle cmd, const RHIPipelineBarrier& barrier);

        // ========================================================================
        // Bind Descriptor Sets with Layout Handle
        // ========================================================================

        // Bind descriptor set using explicit pipeline layout (for when pipeline handle isn't available)
        static void BindDescriptorSet(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                      RHIDescriptorSetHandle set, uint32_t setIndex = 0);

        // Bind raw descriptor set (for bindless - accepts void* for backend-specific handle)
        static void BindDescriptorSetRaw(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                         void* descriptorSet, uint32_t setIndex = 0);
    };

}
