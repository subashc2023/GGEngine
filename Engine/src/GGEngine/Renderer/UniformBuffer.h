#pragma once

#include "GGEngine/Core/Core.h"
#include "Buffer.h"
#include <vulkan/vulkan.h>
#include <memory>

namespace GGEngine {

    class GG_API UniformBuffer
    {
    public:
        UniformBuffer(uint64_t size);
        ~UniformBuffer() = default;

        UniformBuffer(const UniformBuffer&) = delete;
        UniformBuffer& operator=(const UniformBuffer&) = delete;

        // Update data (CPU-visible, no staging needed)
        void SetData(const void* data, uint64_t size, uint64_t offset = 0);

        template<typename T>
        void SetData(const T& data, uint64_t offset = 0)
        {
            SetData(&data, sizeof(T), offset);
        }

        // Accessors
        VkBuffer GetVkBuffer() const { return m_Buffer->GetVkBuffer(); }
        uint64_t GetSize() const { return m_Buffer->GetSize(); }

        // For descriptor set binding
        VkDescriptorBufferInfo GetDescriptorInfo() const;

    private:
        std::unique_ptr<Buffer> m_Buffer;
    };

}
