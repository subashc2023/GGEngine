#pragma once

#include "GGEngine/Core/Core.h"
#include <vulkan/vulkan.h>
#include <cstdint>

namespace GGEngine {

    class GG_API RenderCommand
    {
    public:
        // Viewport
        static void SetViewport(VkCommandBuffer cmd, float x, float y, float width, float height,
                                float minDepth = 0.0f, float maxDepth = 1.0f);
        static void SetViewport(VkCommandBuffer cmd, float width, float height);
        static void SetViewport(VkCommandBuffer cmd, uint32_t width, uint32_t height);

        // Scissor
        static void SetScissor(VkCommandBuffer cmd, int32_t x, int32_t y, uint32_t width, uint32_t height);
        static void SetScissor(VkCommandBuffer cmd, uint32_t width, uint32_t height);

        // Draw commands
        static void Draw(VkCommandBuffer cmd, uint32_t vertexCount, uint32_t instanceCount = 1,
                         uint32_t firstVertex = 0, uint32_t firstInstance = 0);

        static void DrawIndexed(VkCommandBuffer cmd, uint32_t indexCount, uint32_t instanceCount = 1,
                                uint32_t firstIndex = 0, int32_t vertexOffset = 0, uint32_t firstInstance = 0);

        // Push constants
        static void PushConstants(VkCommandBuffer cmd, VkPipelineLayout layout, VkShaderStageFlags stageFlags,
                                  uint32_t offset, uint32_t size, const void* data);

        template<typename T>
        static void PushConstants(VkCommandBuffer cmd, VkPipelineLayout layout, VkShaderStageFlags stageFlags,
                                  const T& data, uint32_t offset = 0)
        {
            PushConstants(cmd, layout, stageFlags, offset, sizeof(T), &data);
        }
    };

}
