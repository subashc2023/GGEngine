#include "ggpch.h"
#include "RenderCommand.h"
#include "Platform/Vulkan/VulkanRHI.h"

namespace GGEngine {

    // ============================================================================
    // RHI Handle-based implementations (delegate to Vk versions)
    // ============================================================================

    void RenderCommand::SetViewport(RHICommandBufferHandle cmd, float x, float y, float width, float height,
                                    float minDepth, float maxDepth)
    {
        auto& registry = VulkanResourceRegistry::Get();
        SetViewportVk(registry.GetCommandBuffer(cmd), x, y, width, height, minDepth, maxDepth);
    }

    void RenderCommand::SetViewport(RHICommandBufferHandle cmd, float width, float height)
    {
        auto& registry = VulkanResourceRegistry::Get();
        SetViewportVk(registry.GetCommandBuffer(cmd), width, height);
    }

    void RenderCommand::SetViewport(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        auto& registry = VulkanResourceRegistry::Get();
        SetViewportVk(registry.GetCommandBuffer(cmd), width, height);
    }

    void RenderCommand::SetScissor(RHICommandBufferHandle cmd, int32_t x, int32_t y, uint32_t width, uint32_t height)
    {
        auto& registry = VulkanResourceRegistry::Get();
        SetScissorVk(registry.GetCommandBuffer(cmd), x, y, width, height);
    }

    void RenderCommand::SetScissor(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        auto& registry = VulkanResourceRegistry::Get();
        SetScissorVk(registry.GetCommandBuffer(cmd), width, height);
    }

    void RenderCommand::Draw(RHICommandBufferHandle cmd, uint32_t vertexCount, uint32_t instanceCount,
                             uint32_t firstVertex, uint32_t firstInstance)
    {
        auto& registry = VulkanResourceRegistry::Get();
        DrawVk(registry.GetCommandBuffer(cmd), vertexCount, instanceCount, firstVertex, firstInstance);
    }

    void RenderCommand::DrawIndexed(RHICommandBufferHandle cmd, uint32_t indexCount, uint32_t instanceCount,
                                    uint32_t firstIndex, int32_t vertexOffset, uint32_t firstInstance)
    {
        auto& registry = VulkanResourceRegistry::Get();
        DrawIndexedVk(registry.GetCommandBuffer(cmd), indexCount, instanceCount, firstIndex, vertexOffset, firstInstance);
    }

    void RenderCommand::PushConstants(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout, ShaderStage stageFlags,
                                      uint32_t offset, uint32_t size, const void* data)
    {
        auto& registry = VulkanResourceRegistry::Get();
        PushConstantsVk(registry.GetCommandBuffer(cmd), registry.GetPipelineLayout(layout), stageFlags, offset, size, data);
    }

    // ============================================================================
    // Vulkan implementations
    // ============================================================================

    void RenderCommand::SetViewportVk(void* vkCmd, float x, float y, float width, float height,
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
        vkCmdSetViewport(static_cast<VkCommandBuffer>(vkCmd), 0, 1, &viewport);
    }

    void RenderCommand::SetViewportVk(void* vkCmd, float width, float height)
    {
        SetViewportVk(vkCmd, 0.0f, 0.0f, width, height, 0.0f, 1.0f);
    }

    void RenderCommand::SetViewportVk(void* vkCmd, uint32_t width, uint32_t height)
    {
        SetViewportVk(vkCmd, 0.0f, 0.0f, static_cast<float>(width), static_cast<float>(height), 0.0f, 1.0f);
    }

    void RenderCommand::SetScissorVk(void* vkCmd, int32_t x, int32_t y, uint32_t width, uint32_t height)
    {
        VkRect2D scissor{};
        scissor.offset = { x, y };
        scissor.extent = { width, height };
        vkCmdSetScissor(static_cast<VkCommandBuffer>(vkCmd), 0, 1, &scissor);
    }

    void RenderCommand::SetScissorVk(void* vkCmd, uint32_t width, uint32_t height)
    {
        SetScissorVk(vkCmd, 0, 0, width, height);
    }

    void RenderCommand::DrawVk(void* vkCmd, uint32_t vertexCount, uint32_t instanceCount,
                               uint32_t firstVertex, uint32_t firstInstance)
    {
        vkCmdDraw(static_cast<VkCommandBuffer>(vkCmd), vertexCount, instanceCount, firstVertex, firstInstance);
    }

    void RenderCommand::DrawIndexedVk(void* vkCmd, uint32_t indexCount, uint32_t instanceCount,
                                      uint32_t firstIndex, int32_t vertexOffset, uint32_t firstInstance)
    {
        vkCmdDrawIndexed(static_cast<VkCommandBuffer>(vkCmd), indexCount, instanceCount, firstIndex, vertexOffset, firstInstance);
    }

    void RenderCommand::PushConstantsVk(void* vkCmd, void* vkPipelineLayout, ShaderStage stageFlags,
                                        uint32_t offset, uint32_t size, const void* data)
    {
        vkCmdPushConstants(static_cast<VkCommandBuffer>(vkCmd), static_cast<VkPipelineLayout>(vkPipelineLayout),
                           ToVulkan(stageFlags), offset, size, data);
    }

}
