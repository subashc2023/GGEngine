#pragma once

#include "GGEngine/Core/Core.h"
#include <vulkan/vulkan.h>
#include <vk_mem_alloc.h>
#include <cstdint>
#include <string>

namespace GGEngine {

    enum class BufferUsage
    {
        Vertex,
        Index,
        Uniform,
        Staging
    };

    struct GG_API BufferSpecification
    {
        uint64_t size = 0;
        BufferUsage usage = BufferUsage::Vertex;
        bool cpuVisible = false;
        std::string debugName;
    };

    class GG_API Buffer
    {
    public:
        Buffer(const BufferSpecification& spec);
        virtual ~Buffer();

        Buffer(const Buffer&) = delete;
        Buffer& operator=(const Buffer&) = delete;

        // Upload data to GPU (uses staging buffer if not CPU-visible)
        void SetData(const void* data, uint64_t size, uint64_t offset = 0);

        // Accessors
        VkBuffer GetVkBuffer() const { return m_Buffer; }
        uint64_t GetSize() const { return m_Specification.size; }
        const BufferSpecification& GetSpecification() const { return m_Specification; }

    protected:
        void Create();
        void Destroy();
        void CopyBuffer(VkBuffer srcBuffer, VkBuffer dstBuffer, VkDeviceSize size, VkDeviceSize dstOffset = 0);

        BufferSpecification m_Specification;
        VkBuffer m_Buffer = VK_NULL_HANDLE;
        VmaAllocation m_Allocation = VK_NULL_HANDLE;
    };

}
