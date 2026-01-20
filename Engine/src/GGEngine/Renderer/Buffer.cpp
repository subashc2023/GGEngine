#include "ggpch.h"
#include "Buffer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Log.h"
#include <cstring>

namespace GGEngine {

    Buffer::Buffer(const BufferSpecification& spec)
        : m_Specification(spec)
    {
        Create();
    }

    Buffer::~Buffer()
    {
        Destroy();
    }

    void Buffer::Create()
    {
        if (m_Specification.size == 0)
        {
            GG_CORE_ERROR("Buffer creation failed: size is 0");
            return;
        }

        auto allocator = VulkanContext::Get().GetAllocator();

        VkBufferCreateInfo bufferInfo{};
        bufferInfo.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
        bufferInfo.size = m_Specification.size;
        bufferInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;

        switch (m_Specification.usage)
        {
            case BufferUsage::Vertex:
                bufferInfo.usage = VK_BUFFER_USAGE_VERTEX_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT;
                break;
            case BufferUsage::Index:
                bufferInfo.usage = VK_BUFFER_USAGE_INDEX_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT;
                break;
            case BufferUsage::Uniform:
                bufferInfo.usage = VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT;
                break;
            case BufferUsage::Staging:
                bufferInfo.usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT;
                break;
        }

        VmaAllocationCreateInfo allocInfo{};
        allocInfo.usage = VMA_MEMORY_USAGE_AUTO;

        if (m_Specification.cpuVisible)
        {
            allocInfo.flags = VMA_ALLOCATION_CREATE_HOST_ACCESS_SEQUENTIAL_WRITE_BIT |
                              VMA_ALLOCATION_CREATE_MAPPED_BIT;
        }
        else
        {
            allocInfo.flags = VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT;
        }

        if (vmaCreateBuffer(allocator, &bufferInfo, &allocInfo, &m_Buffer, &m_Allocation, nullptr) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create buffer with VMA!");
            return;
        }

        if (!m_Specification.debugName.empty())
        {
            GG_CORE_TRACE("Buffer '{}' created ({} bytes)", m_Specification.debugName, m_Specification.size);
        }
    }

    void Buffer::Destroy()
    {
        if (m_Buffer != VK_NULL_HANDLE)
        {
            auto allocator = VulkanContext::Get().GetAllocator();
            vmaDestroyBuffer(allocator, m_Buffer, m_Allocation);
            m_Buffer = VK_NULL_HANDLE;
            m_Allocation = VK_NULL_HANDLE;
        }
    }

    void Buffer::SetData(const void* data, uint64_t size, uint64_t offset)
    {
        if (size == 0 || data == nullptr)
            return;

        if (offset + size > m_Specification.size)
        {
            GG_CORE_ERROR("Buffer::SetData: data exceeds buffer size");
            return;
        }

        if (m_Specification.cpuVisible)
        {
            // Direct write for CPU-visible buffers
            void* mapped = nullptr;
            if (vmaMapMemory(VulkanContext::Get().GetAllocator(), m_Allocation, &mapped) == VK_SUCCESS)
            {
                std::memcpy(static_cast<char*>(mapped) + offset, data, size);
                vmaUnmapMemory(VulkanContext::Get().GetAllocator(), m_Allocation);
            }
            else
            {
                GG_CORE_ERROR("Failed to map buffer memory");
            }
        }
        else
        {
            // Use staging buffer for GPU-only memory
            BufferSpecification stagingSpec;
            stagingSpec.size = size;
            stagingSpec.usage = BufferUsage::Staging;
            stagingSpec.cpuVisible = true;

            Buffer stagingBuffer(stagingSpec);
            stagingBuffer.SetData(data, size);

            CopyBuffer(stagingBuffer.GetVkBuffer(), m_Buffer, size);
        }
    }

    void Buffer::CopyBuffer(VkBuffer srcBuffer, VkBuffer dstBuffer, VkDeviceSize size)
    {
        auto& vkContext = VulkanContext::Get();
        auto device = vkContext.GetDevice();
        auto commandPool = vkContext.GetCommandPool();
        auto graphicsQueue = vkContext.GetGraphicsQueue();

        VkCommandBufferAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
        allocInfo.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
        allocInfo.commandPool = commandPool;
        allocInfo.commandBufferCount = 1;

        VkCommandBuffer commandBuffer;
        vkAllocateCommandBuffers(device, &allocInfo, &commandBuffer);

        VkCommandBufferBeginInfo beginInfo{};
        beginInfo.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
        beginInfo.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;

        vkBeginCommandBuffer(commandBuffer, &beginInfo);

        VkBufferCopy copyRegion{};
        copyRegion.srcOffset = 0;
        copyRegion.dstOffset = 0;
        copyRegion.size = size;
        vkCmdCopyBuffer(commandBuffer, srcBuffer, dstBuffer, 1, &copyRegion);

        vkEndCommandBuffer(commandBuffer);

        VkSubmitInfo submitInfo{};
        submitInfo.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
        submitInfo.commandBufferCount = 1;
        submitInfo.pCommandBuffers = &commandBuffer;

        vkQueueSubmit(graphicsQueue, 1, &submitInfo, VK_NULL_HANDLE);
        vkQueueWaitIdle(graphicsQueue);

        vkFreeCommandBuffers(device, commandPool, 1, &commandBuffer);
    }

}
