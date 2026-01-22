#include "ggpch.h"
#include "Buffer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Core/Log.h"
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
            VmaAllocator allocator = VulkanContext::Get().GetAllocator();
            if (vmaMapMemory(allocator, m_Allocation, &mapped) == VK_SUCCESS)
            {
                std::memcpy(static_cast<char*>(mapped) + offset, data, size);
                // Flush to ensure GPU can see the data (required for non-coherent memory)
                vmaFlushAllocation(allocator, m_Allocation, offset, size);
                vmaUnmapMemory(allocator, m_Allocation);
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

            CopyBuffer(stagingBuffer.GetVkBuffer(), m_Buffer, size, offset);
        }
    }

    void Buffer::CopyBuffer(VkBuffer srcBuffer, VkBuffer dstBuffer, VkDeviceSize size, VkDeviceSize dstOffset)
    {
        VulkanContext::Get().ImmediateSubmit([=](VkCommandBuffer cmd) {
            VkBufferCopy copyRegion{};
            copyRegion.srcOffset = 0;
            copyRegion.dstOffset = dstOffset;  // Use the provided destination offset
            copyRegion.size = size;
            vkCmdCopyBuffer(cmd, srcBuffer, dstBuffer, 1, &copyRegion);

            // Ensure transfer writes are visible to subsequent reads on this queue.
            VkAccessFlags dstAccess = 0;
            VkPipelineStageFlags dstStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
            switch (m_Specification.usage)
            {
                case BufferUsage::Vertex:
                    dstAccess = VK_ACCESS_VERTEX_ATTRIBUTE_READ_BIT;
                    dstStage = VK_PIPELINE_STAGE_VERTEX_INPUT_BIT;
                    break;
                case BufferUsage::Index:
                    dstAccess = VK_ACCESS_INDEX_READ_BIT;
                    dstStage = VK_PIPELINE_STAGE_VERTEX_INPUT_BIT;
                    break;
                case BufferUsage::Uniform:
                    dstAccess = VK_ACCESS_UNIFORM_READ_BIT;
                    dstStage = VK_PIPELINE_STAGE_VERTEX_SHADER_BIT | VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
                    break;
                case BufferUsage::Staging:
                    dstAccess = 0;
                    dstStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
                    break;
            }

            if (dstAccess != 0)
            {
                VkBufferMemoryBarrier barrier{};
                barrier.sType = VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER;
                barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
                barrier.dstAccessMask = dstAccess;
                barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
                barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
                barrier.buffer = dstBuffer;
                barrier.offset = dstOffset;  // Barrier should cover the written region
                barrier.size = size;

                vkCmdPipelineBarrier(
                    cmd,
                    VK_PIPELINE_STAGE_TRANSFER_BIT,
                    dstStage,
                    0,
                    0, nullptr,
                    1, &barrier,
                    0, nullptr);
            }
        });
    }

}
