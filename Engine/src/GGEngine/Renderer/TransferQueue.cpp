#include "ggpch.h"
#include "TransferQueue.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "Platform/Vulkan/VulkanRHI.h"
#include "Platform/Vulkan/VulkanContext.h"

#include <vulkan/vulkan.h>

namespace GGEngine {

    TransferQueue& TransferQueue::Get()
    {
        static TransferQueue instance;
        return instance;
    }

    void TransferQueue::QueueTextureUpload(
        RHITextureHandle texture,
        const void* data,
        uint64_t size,
        uint32_t width,
        uint32_t height,
        UploadCompleteCallback callback)
    {
        if (!texture.IsValid() || !data || size == 0)
        {
            GG_CORE_WARN("TransferQueue::QueueTextureUpload - invalid parameters");
            return;
        }

        auto& device = RHIDevice::Get();

        // Create staging buffer and copy data immediately (can be called from any thread)
        RHIBufferSpecification stagingSpec;
        stagingSpec.size = size;
        stagingSpec.usage = BufferUsage::Staging;
        stagingSpec.cpuVisible = true;

        RHIBufferHandle stagingBuffer = device.CreateBuffer(stagingSpec);
        if (!stagingBuffer.IsValid())
        {
            GG_CORE_ERROR("TransferQueue::QueueTextureUpload - failed to create staging buffer");
            return;
        }

        // Copy data to staging buffer
        device.UploadBufferData(stagingBuffer, data, size, 0);

        // Queue the upload request
        {
            std::lock_guard<std::mutex> lock(m_Mutex);
            m_PendingTextureUploads.push_back({
                texture,
                stagingBuffer,
                width,
                height,
                std::move(callback)
            });
        }

        GG_CORE_TRACE("TransferQueue: queued texture upload ({}x{}, {} bytes)", width, height, size);
    }

    void TransferQueue::QueueBufferUpload(
        RHIBufferHandle buffer,
        const void* data,
        uint64_t size,
        uint64_t offset,
        UploadCompleteCallback callback)
    {
        if (!buffer.IsValid() || !data || size == 0)
        {
            GG_CORE_WARN("TransferQueue::QueueBufferUpload - invalid parameters");
            return;
        }

        auto& device = RHIDevice::Get();

        // Create staging buffer
        RHIBufferSpecification stagingSpec;
        stagingSpec.size = size;
        stagingSpec.usage = BufferUsage::Staging;
        stagingSpec.cpuVisible = true;

        RHIBufferHandle stagingBuffer = device.CreateBuffer(stagingSpec);
        if (!stagingBuffer.IsValid())
        {
            GG_CORE_ERROR("TransferQueue::QueueBufferUpload - failed to create staging buffer");
            return;
        }

        device.UploadBufferData(stagingBuffer, data, size, 0);

        {
            std::lock_guard<std::mutex> lock(m_Mutex);
            m_PendingBufferUploads.push_back({
                buffer,
                stagingBuffer,
                size,
                offset,
                std::move(callback)
            });
        }

        GG_CORE_TRACE("TransferQueue: queued buffer upload ({} bytes at offset {})", size, offset);
    }

    void TransferQueue::FlushUploads(RHICommandBufferHandle cmd)
    {
        // Swap out pending uploads to minimize lock time
        std::vector<TextureUploadRequest> textureUploads;
        std::vector<BufferUploadRequest> bufferUploads;

        {
            std::lock_guard<std::mutex> lock(m_Mutex);
            std::swap(textureUploads, m_PendingTextureUploads);
            std::swap(bufferUploads, m_PendingBufferUploads);
        }

        if (textureUploads.empty() && bufferUploads.empty())
            return;

        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = VulkanContext::Get().GetCurrentCommandBuffer();
        uint32_t frameIndex = RHIDevice::Get().GetCurrentFrameIndex();

        // Process texture uploads
        for (auto& request : textureUploads)
        {
            auto textureData = registry.GetTextureData(request.texture);
            VkBuffer stagingVkBuffer = registry.GetBuffer(request.stagingBuffer);

            // Transition to transfer dst
            VkImageMemoryBarrier barrier{};
            barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            barrier.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
            barrier.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.image = textureData.image;
            barrier.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            barrier.subresourceRange.baseMipLevel = 0;
            barrier.subresourceRange.levelCount = 1;
            barrier.subresourceRange.baseArrayLayer = 0;
            barrier.subresourceRange.layerCount = 1;
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;

            vkCmdPipelineBarrier(vkCmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                 0, 0, nullptr, 0, nullptr, 1, &barrier);

            // Copy buffer to image
            VkBufferImageCopy region{};
            region.bufferOffset = 0;
            region.bufferRowLength = 0;
            region.bufferImageHeight = 0;
            region.imageSubresource.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            region.imageSubresource.mipLevel = 0;
            region.imageSubresource.baseArrayLayer = 0;
            region.imageSubresource.layerCount = 1;
            region.imageOffset = { 0, 0, 0 };
            region.imageExtent = { request.width, request.height, 1 };

            vkCmdCopyBufferToImage(vkCmd, stagingVkBuffer, textureData.image,
                                   VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);

            // Transition to shader read
            barrier.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
            barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;

            vkCmdPipelineBarrier(vkCmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                                 0, 0, nullptr, 0, nullptr, 1, &barrier);

            // Track staging buffer for cleanup after frame completes
            m_StagingBuffersInFlight[frameIndex].push_back(request.stagingBuffer);

            // Track callback
            if (request.callback)
                m_PendingCallbacks[frameIndex].push_back(std::move(request.callback));
        }

        // Process buffer uploads
        for (auto& request : bufferUploads)
        {
            VkBuffer stagingVkBuffer = registry.GetBuffer(request.stagingBuffer);
            VkBuffer targetVkBuffer = registry.GetBuffer(request.target);

            VkBufferCopy copyRegion{};
            copyRegion.srcOffset = 0;
            copyRegion.dstOffset = request.offset;
            copyRegion.size = request.size;

            vkCmdCopyBuffer(vkCmd, stagingVkBuffer, targetVkBuffer, 1, &copyRegion);

            m_StagingBuffersInFlight[frameIndex].push_back(request.stagingBuffer);

            if (request.callback)
                m_PendingCallbacks[frameIndex].push_back(std::move(request.callback));
        }

        GG_CORE_TRACE("TransferQueue: flushed {} texture and {} buffer uploads",
                      textureUploads.size(), bufferUploads.size());
    }

    void TransferQueue::EndFrame(uint32_t frameIndex)
    {
        auto& device = RHIDevice::Get();

        // Destroy staging buffers from the frame that just completed
        // (we're now starting a new frame with this index, so the previous
        // frame's GPU work is guaranteed complete due to fence wait in BeginFrame)
        for (auto& buffer : m_StagingBuffersInFlight[frameIndex])
        {
            device.DestroyBuffer(buffer);
        }
        m_StagingBuffersInFlight[frameIndex].clear();

        // Fire callbacks for completed uploads
        for (auto& callback : m_PendingCallbacks[frameIndex])
        {
            if (callback)
                callback();
        }
        m_PendingCallbacks[frameIndex].clear();
    }

    uint32_t TransferQueue::GetPendingCount() const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        return static_cast<uint32_t>(m_PendingTextureUploads.size() + m_PendingBufferUploads.size());
    }

    void TransferQueue::Shutdown()
    {
        auto& device = RHIDevice::Get();

        // Cleanup any remaining staging buffers
        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            for (auto& buffer : m_StagingBuffersInFlight[i])
            {
                device.DestroyBuffer(buffer);
            }
            m_StagingBuffersInFlight[i].clear();
            m_PendingCallbacks[i].clear();
        }

        // Clear pending uploads (shouldn't happen in normal shutdown)
        {
            std::lock_guard<std::mutex> lock(m_Mutex);
            for (auto& request : m_PendingTextureUploads)
            {
                device.DestroyBuffer(request.stagingBuffer);
            }
            m_PendingTextureUploads.clear();

            for (auto& request : m_PendingBufferUploads)
            {
                device.DestroyBuffer(request.stagingBuffer);
            }
            m_PendingBufferUploads.clear();
        }

        GG_CORE_TRACE("TransferQueue shutdown");
    }

}
