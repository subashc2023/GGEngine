#include "ggpch.h"
#include "RenderCommand.h"

namespace GGEngine {

    void RenderCommand::SetViewport(VkCommandBuffer cmd, float x, float y, float width, float height,
                                    float minDepth, float maxDepth)
    {
        VkViewport viewport{};
        viewport.x = x;
        // Flip Y using negative height (Vulkan 1.1+ / VK_KHR_maintenance1)
        viewport.y = y + height;
        viewport.width = width;
        viewport.height = -height;
        viewport.minDepth = minDepth;
        viewport.maxDepth = maxDepth;
        vkCmdSetViewport(cmd, 0, 1, &viewport);
    }

    void RenderCommand::SetViewport(VkCommandBuffer cmd, float width, float height)
    {
        SetViewport(cmd, 0.0f, 0.0f, width, height, 0.0f, 1.0f);
    }

    void RenderCommand::SetViewport(VkCommandBuffer cmd, uint32_t width, uint32_t height)
    {
        SetViewport(cmd, 0.0f, 0.0f, static_cast<float>(width), static_cast<float>(height), 0.0f, 1.0f);
    }

    void RenderCommand::SetScissor(VkCommandBuffer cmd, int32_t x, int32_t y, uint32_t width, uint32_t height)
    {
        VkRect2D scissor{};
        scissor.offset = { x, y };
        scissor.extent = { width, height };
        vkCmdSetScissor(cmd, 0, 1, &scissor);
    }

    void RenderCommand::SetScissor(VkCommandBuffer cmd, uint32_t width, uint32_t height)
    {
        SetScissor(cmd, 0, 0, width, height);
    }

    void RenderCommand::Draw(VkCommandBuffer cmd, uint32_t vertexCount, uint32_t instanceCount,
                             uint32_t firstVertex, uint32_t firstInstance)
    {
        vkCmdDraw(cmd, vertexCount, instanceCount, firstVertex, firstInstance);
    }

    void RenderCommand::DrawIndexed(VkCommandBuffer cmd, uint32_t indexCount, uint32_t instanceCount,
                                    uint32_t firstIndex, int32_t vertexOffset, uint32_t firstInstance)
    {
        vkCmdDrawIndexed(cmd, indexCount, instanceCount, firstIndex, vertexOffset, firstInstance);
    }

    void RenderCommand::PushConstants(VkCommandBuffer cmd, VkPipelineLayout layout, VkShaderStageFlags stageFlags,
                                      uint32_t offset, uint32_t size, const void* data)
    {
        vkCmdPushConstants(cmd, layout, stageFlags, offset, size, data);
    }

}
