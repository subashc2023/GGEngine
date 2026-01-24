#include "ggpch.h"
#include "VulkanResourceRegistry.h"
#include "VulkanConversions.h"
#include "GGEngine/RHI/RHICommandBuffer.h"

namespace GGEngine {

    // ============================================================================
    // RHICmd Implementation
    // ============================================================================

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, float x, float y, float width, float height,
                             float minDepth, float maxDepth)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        VkViewport viewport{};
        viewport.x = x;
        viewport.y = height;  // Flip Y for Vulkan coordinate system
        viewport.width = width;
        viewport.height = -height;  // Negative height for Y-flip
        viewport.minDepth = minDepth;
        viewport.maxDepth = maxDepth;
        vkCmdSetViewport(vkCmd, 0, 1, &viewport);
    }

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, float width, float height)
    {
        SetViewport(cmd, 0.0f, 0.0f, width, height, 0.0f, 1.0f);
    }

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        SetViewport(cmd, 0.0f, 0.0f, static_cast<float>(width), static_cast<float>(height), 0.0f, 1.0f);
    }

    void RHICmd::SetScissor(RHICommandBufferHandle cmd, int32_t x, int32_t y, uint32_t width, uint32_t height)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        VkRect2D scissor{};
        scissor.offset = { x, y };
        scissor.extent = { width, height };
        vkCmdSetScissor(vkCmd, 0, 1, &scissor);
    }

    void RHICmd::SetScissor(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        SetScissor(cmd, 0, 0, width, height);
    }

    void RHICmd::BindPipeline(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipeline vkPipeline = registry.GetPipeline(pipeline);
        if (vkCmd == VK_NULL_HANDLE || vkPipeline == VK_NULL_HANDLE) return;

        vkCmdBindPipeline(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, vkPipeline);
    }

    void RHICmd::BindVertexBuffer(RHICommandBufferHandle cmd, RHIBufferHandle buffer, uint32_t binding)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer vkBuffer = registry.GetBuffer(buffer);
        if (vkCmd == VK_NULL_HANDLE || vkBuffer == VK_NULL_HANDLE) return;

        VkDeviceSize offset = 0;
        vkCmdBindVertexBuffers(vkCmd, binding, 1, &vkBuffer, &offset);
    }

    void RHICmd::BindIndexBuffer(RHICommandBufferHandle cmd, RHIBufferHandle buffer, IndexType indexType)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer vkBuffer = registry.GetBuffer(buffer);
        if (vkCmd == VK_NULL_HANDLE || vkBuffer == VK_NULL_HANDLE) return;

        vkCmdBindIndexBuffer(vkCmd, vkBuffer, 0, ToVulkan(indexType));
    }

    void RHICmd::BindDescriptorSet(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                                   RHIDescriptorSetHandle set, uint32_t setIndex)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout layout = registry.GetPipelineLayout(pipeline);
        VkDescriptorSet vkSet = registry.GetDescriptorSet(set);
        if (vkCmd == VK_NULL_HANDLE || layout == VK_NULL_HANDLE || vkSet == VK_NULL_HANDLE) return;

        vkCmdBindDescriptorSets(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, layout, setIndex, 1, &vkSet, 0, nullptr);
    }

    void RHICmd::PushConstants(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                               ShaderStage stages, uint32_t offset, uint32_t size, const void* data)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout layout = registry.GetPipelineLayout(pipeline);
        if (vkCmd == VK_NULL_HANDLE || layout == VK_NULL_HANDLE) return;

        vkCmdPushConstants(vkCmd, layout, ToVulkan(stages), offset, size, data);
    }

    void RHICmd::PushConstants(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layoutHandle,
                               ShaderStage stages, uint32_t offset, uint32_t size, const void* data)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout layout = registry.GetPipelineLayout(layoutHandle);
        if (vkCmd == VK_NULL_HANDLE || layout == VK_NULL_HANDLE) return;

        vkCmdPushConstants(vkCmd, layout, ToVulkan(stages), offset, size, data);
    }

    void RHICmd::Draw(RHICommandBufferHandle cmd, uint32_t vertexCount, uint32_t instanceCount,
                      uint32_t firstVertex, uint32_t firstInstance)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        vkCmdDraw(vkCmd, vertexCount, instanceCount, firstVertex, firstInstance);
    }

    void RHICmd::DrawIndexed(RHICommandBufferHandle cmd, uint32_t indexCount, uint32_t instanceCount,
                             uint32_t firstIndex, int32_t vertexOffset, uint32_t firstInstance)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        vkCmdDrawIndexed(vkCmd, indexCount, instanceCount, firstIndex, vertexOffset, firstInstance);
    }

    void RHICmd::BeginRenderPass(RHICommandBufferHandle cmd, RHIRenderPassHandle renderPass,
                                 RHIFramebufferHandle framebuffer, uint32_t width, uint32_t height,
                                 float clearR, float clearG, float clearB, float clearA)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        auto rpData = registry.GetRenderPassData(renderPass);

        if (vkCmd == VK_NULL_HANDLE || rpData.renderPass == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("BeginRenderPass: Invalid command buffer or render pass");
            return;
        }

        // Use explicit framebuffer handle if provided, otherwise fall back to render pass data
        VkFramebuffer vkFramebuffer = framebuffer.IsValid()
            ? reinterpret_cast<VkFramebuffer>(framebuffer.id)
            : rpData.framebuffer;

        if (vkFramebuffer == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("BeginRenderPass: Invalid framebuffer");
            return;
        }

        VkRenderPassBeginInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO;
        renderPassInfo.renderPass = rpData.renderPass;
        renderPassInfo.framebuffer = vkFramebuffer;
        renderPassInfo.renderArea.offset = { 0, 0 };
        renderPassInfo.renderArea.extent = { width, height };

        VkClearValue clearValue{};
        clearValue.color = {{ clearR, clearG, clearB, clearA }};
        renderPassInfo.clearValueCount = 1;
        renderPassInfo.pClearValues = &clearValue;

        vkCmdBeginRenderPass(vkCmd, &renderPassInfo, VK_SUBPASS_CONTENTS_INLINE);
    }

    void RHICmd::EndRenderPass(RHICommandBufferHandle cmd)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        vkCmdEndRenderPass(vkCmd);
    }

    // ============================================================================
    // RHICmd: Transfer Commands
    // ============================================================================

    void RHICmd::CopyBuffer(RHICommandBufferHandle cmd, RHIBufferHandle src, RHIBufferHandle dst,
                            uint64_t srcOffset, uint64_t dstOffset, uint64_t size)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer srcBuffer = registry.GetBuffer(src);
        VkBuffer dstBuffer = registry.GetBuffer(dst);
        if (vkCmd == VK_NULL_HANDLE || srcBuffer == VK_NULL_HANDLE || dstBuffer == VK_NULL_HANDLE) return;

        VkBufferCopy copyRegion{};
        copyRegion.srcOffset = srcOffset;
        copyRegion.dstOffset = dstOffset;
        copyRegion.size = size;

        vkCmdCopyBuffer(vkCmd, srcBuffer, dstBuffer, 1, &copyRegion);
    }

    void RHICmd::CopyBufferToTexture(RHICommandBufferHandle cmd, RHIBufferHandle buffer,
                                     RHITextureHandle texture, const RHIBufferImageCopy& region)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer vkBuffer = registry.GetBuffer(buffer);
        auto texData = registry.GetTextureData(texture);
        if (vkCmd == VK_NULL_HANDLE || vkBuffer == VK_NULL_HANDLE || texData.image == VK_NULL_HANDLE) return;

        VkBufferImageCopy vkRegion{};
        vkRegion.bufferOffset = region.bufferOffset;
        vkRegion.bufferRowLength = region.bufferRowLength;
        vkRegion.bufferImageHeight = region.bufferImageHeight;
        vkRegion.imageSubresource.aspectMask = IsDepthFormat(texData.format) ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
        vkRegion.imageSubresource.mipLevel = region.mipLevel;
        vkRegion.imageSubresource.baseArrayLayer = region.arrayLayer;
        vkRegion.imageSubresource.layerCount = region.layerCount;
        vkRegion.imageOffset = { static_cast<int32_t>(region.imageOffsetX),
                                 static_cast<int32_t>(region.imageOffsetY),
                                 static_cast<int32_t>(region.imageOffsetZ) };
        vkRegion.imageExtent = { region.imageWidth, region.imageHeight, region.imageDepth };

        vkCmdCopyBufferToImage(vkCmd, vkBuffer, texData.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &vkRegion);
    }

    void RHICmd::CopyBufferToTexture(RHICommandBufferHandle cmd, RHIBufferHandle buffer,
                                     RHITextureHandle texture, uint32_t width, uint32_t height)
    {
        RHIBufferImageCopy region{};
        region.bufferOffset = 0;
        region.bufferRowLength = 0;
        region.bufferImageHeight = 0;
        region.imageOffsetX = 0;
        region.imageOffsetY = 0;
        region.imageOffsetZ = 0;
        region.imageWidth = width;
        region.imageHeight = height;
        region.imageDepth = 1;
        region.mipLevel = 0;
        region.arrayLayer = 0;
        region.layerCount = 1;

        CopyBufferToTexture(cmd, buffer, texture, region);
    }

    // ============================================================================
    // RHICmd: Image Layout Transitions
    // ============================================================================

    void RHICmd::TransitionImageLayout(RHICommandBufferHandle cmd, RHITextureHandle texture,
                                       ImageLayout oldLayout, ImageLayout newLayout)
    {
        auto& registry = VulkanResourceRegistry::Get();
        auto texData = registry.GetTextureData(texture);

        TransitionImageLayout(cmd, texture, oldLayout, newLayout, 0, 1, 0, 1);
    }

    void RHICmd::TransitionImageLayout(RHICommandBufferHandle cmd, RHITextureHandle texture,
                                       ImageLayout oldLayout, ImageLayout newLayout,
                                       uint32_t baseMipLevel, uint32_t mipCount,
                                       uint32_t baseArrayLayer, uint32_t layerCount)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        auto texData = registry.GetTextureData(texture);
        if (vkCmd == VK_NULL_HANDLE || texData.image == VK_NULL_HANDLE) return;

        VkImageMemoryBarrier barrier{};
        barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
        barrier.oldLayout = ToVulkan(oldLayout);
        barrier.newLayout = ToVulkan(newLayout);
        barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        barrier.image = texData.image;
        barrier.subresourceRange.aspectMask = IsDepthFormat(texData.format) ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
        barrier.subresourceRange.baseMipLevel = baseMipLevel;
        barrier.subresourceRange.levelCount = mipCount;
        barrier.subresourceRange.baseArrayLayer = baseArrayLayer;
        barrier.subresourceRange.layerCount = layerCount;

        VkPipelineStageFlags srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
        VkPipelineStageFlags dstStage = VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT;

        // Determine access masks and pipeline stages based on layouts
        if (oldLayout == ImageLayout::Undefined && newLayout == ImageLayout::TransferDst)
        {
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
            dstStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
        }
        else if (oldLayout == ImageLayout::TransferDst && newLayout == ImageLayout::ShaderReadOnly)
        {
            barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
            srcStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
            dstStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
        }
        else if (oldLayout == ImageLayout::Undefined && newLayout == ImageLayout::ColorAttachment)
        {
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
            dstStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        }
        else if (oldLayout == ImageLayout::ColorAttachment && newLayout == ImageLayout::ShaderReadOnly)
        {
            barrier.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
            srcStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            dstStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
        }
        else if (oldLayout == ImageLayout::ShaderReadOnly && newLayout == ImageLayout::ColorAttachment)
        {
            barrier.srcAccessMask = VK_ACCESS_SHADER_READ_BIT;
            barrier.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            dstStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        }
        else if (oldLayout == ImageLayout::Undefined && newLayout == ImageLayout::DepthStencilAttachment)
        {
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
            dstStage = VK_PIPELINE_STAGE_EARLY_FRAGMENT_TESTS_BIT;
        }
        else if (oldLayout == ImageLayout::ShaderReadOnly && newLayout == ImageLayout::TransferDst)
        {
            barrier.srcAccessMask = VK_ACCESS_SHADER_READ_BIT;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            dstStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
        }
        else if (oldLayout == ImageLayout::ColorAttachment && newLayout == ImageLayout::Present)
        {
            barrier.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            barrier.dstAccessMask = 0;
            srcStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            dstStage = VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT;
        }
        else
        {
            // Generic fallback - all stages
            barrier.srcAccessMask = VK_ACCESS_MEMORY_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_MEMORY_READ_BIT | VK_ACCESS_MEMORY_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
            dstStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
        }

        vkCmdPipelineBarrier(vkCmd, srcStage, dstStage, 0,
                             0, nullptr, 0, nullptr, 1, &barrier);
    }

    void RHICmd::PipelineBarrier(RHICommandBufferHandle cmd, const RHIPipelineBarrier& barrier)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        std::vector<VkImageMemoryBarrier> imageBarriers;
        imageBarriers.reserve(barrier.imageBarriers.size());

        VkPipelineStageFlags srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
        VkPipelineStageFlags dstStage = VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT;

        for (const auto& imgBarrier : barrier.imageBarriers)
        {
            auto texData = registry.GetTextureData(imgBarrier.texture);
            if (texData.image == VK_NULL_HANDLE) continue;

            VkImageMemoryBarrier vkBarrier{};
            vkBarrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            vkBarrier.oldLayout = ToVulkan(imgBarrier.oldLayout);
            vkBarrier.newLayout = ToVulkan(imgBarrier.newLayout);
            vkBarrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            vkBarrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            vkBarrier.image = texData.image;
            vkBarrier.subresourceRange.aspectMask = IsDepthFormat(texData.format) ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
            vkBarrier.subresourceRange.baseMipLevel = imgBarrier.baseMipLevel;
            vkBarrier.subresourceRange.levelCount = imgBarrier.mipCount;
            vkBarrier.subresourceRange.baseArrayLayer = imgBarrier.baseArrayLayer;
            vkBarrier.subresourceRange.layerCount = imgBarrier.layerCount;

            // Simplified access mask determination
            if (imgBarrier.oldLayout == ImageLayout::Undefined)
            {
                vkBarrier.srcAccessMask = 0;
                srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
            }
            else if (imgBarrier.oldLayout == ImageLayout::TransferDst)
            {
                vkBarrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
                srcStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
            }
            else if (imgBarrier.oldLayout == ImageLayout::ColorAttachment)
            {
                vkBarrier.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
                srcStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            }
            else
            {
                vkBarrier.srcAccessMask = VK_ACCESS_MEMORY_READ_BIT | VK_ACCESS_MEMORY_WRITE_BIT;
                srcStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
            }

            if (imgBarrier.newLayout == ImageLayout::ShaderReadOnly)
            {
                vkBarrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
                dstStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            }
            else if (imgBarrier.newLayout == ImageLayout::TransferDst)
            {
                vkBarrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
                dstStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
            }
            else if (imgBarrier.newLayout == ImageLayout::ColorAttachment)
            {
                vkBarrier.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
                dstStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            }
            else
            {
                vkBarrier.dstAccessMask = VK_ACCESS_MEMORY_READ_BIT | VK_ACCESS_MEMORY_WRITE_BIT;
                dstStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
            }

            imageBarriers.push_back(vkBarrier);
        }

        if (!imageBarriers.empty())
        {
            vkCmdPipelineBarrier(vkCmd, srcStage, dstStage, 0,
                                 0, nullptr, 0, nullptr,
                                 static_cast<uint32_t>(imageBarriers.size()), imageBarriers.data());
        }
    }

    // ============================================================================
    // RHICmd: Descriptor Set Binding (with pipeline layout handle)
    // ============================================================================

    void RHICmd::BindDescriptorSet(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                   RHIDescriptorSetHandle set, uint32_t setIndex)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout vkLayout = registry.GetPipelineLayout(layout);
        VkDescriptorSet vkSet = registry.GetDescriptorSet(set);
        if (vkCmd == VK_NULL_HANDLE || vkLayout == VK_NULL_HANDLE || vkSet == VK_NULL_HANDLE) return;

        vkCmdBindDescriptorSets(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, vkLayout, setIndex, 1, &vkSet, 0, nullptr);
    }

    void RHICmd::BindDescriptorSetRaw(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                      void* descriptorSet, uint32_t setIndex)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout vkLayout = registry.GetPipelineLayout(layout);
        VkDescriptorSet vkSet = static_cast<VkDescriptorSet>(descriptorSet);
        if (vkCmd == VK_NULL_HANDLE || vkLayout == VK_NULL_HANDLE || vkSet == VK_NULL_HANDLE) return;

        vkCmdBindDescriptorSets(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, vkLayout, setIndex, 1, &vkSet, 0, nullptr);
    }

}
