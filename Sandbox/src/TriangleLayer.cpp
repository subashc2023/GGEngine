#include "TriangleLayer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Renderer/Pipeline.h"

#include <vulkan/vulkan.h>

TriangleLayer::TriangleLayer()
    : Layer("TriangleLayer")
{
}

void TriangleLayer::OnAttach()
{
    GG_INFO("TriangleLayer attached");
}

void TriangleLayer::OnDetach()
{
    GG_INFO("TriangleLayer detached");
}

void TriangleLayer::OnUpdate()
{
    auto& vkContext = GGEngine::VulkanContext::Get();
    VkCommandBuffer cmd = vkContext.GetCurrentCommandBuffer();
    auto* pipeline = vkContext.GetTrianglePipeline();

    // Check if we have a valid command buffer and pipeline
    if (cmd == VK_NULL_HANDLE || !pipeline)
        return;

    // Bind the triangle pipeline
    pipeline->Bind(cmd);

    // Set dynamic viewport and scissor
    VkExtent2D extent = vkContext.GetSwapchainExtent();

    VkViewport viewport{};
    viewport.x = 0.0f;
    viewport.y = 0.0f;
    viewport.width = static_cast<float>(extent.width);
    viewport.height = static_cast<float>(extent.height);
    viewport.minDepth = 0.0f;
    viewport.maxDepth = 1.0f;
    vkCmdSetViewport(cmd, 0, 1, &viewport);

    VkRect2D scissor{};
    scissor.offset = {0, 0};
    scissor.extent = extent;
    vkCmdSetScissor(cmd, 0, 1, &scissor);

    // Draw 3 vertices (hardcoded in shader)
    vkCmdDraw(cmd, 3, 1, 0, 0);
}

void TriangleLayer::OnEvent(GGEngine::Event& event)
{
    // Handle events if needed
}
