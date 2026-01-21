#pragma once

#include "GGEngine/Core/Core.h"
#include "Buffer.h"
#include <vulkan/vulkan.h>
#include <vector>

namespace GGEngine {

    class GG_API IndexBuffer
    {
    public:
        // Create from 32-bit indices
        IndexBuffer(const uint32_t* indices, uint32_t count);

        // Create from 16-bit indices
        IndexBuffer(const uint16_t* indices, uint32_t count);

        ~IndexBuffer() = default;

        IndexBuffer(const IndexBuffer&) = delete;
        IndexBuffer& operator=(const IndexBuffer&) = delete;

        // Factory methods
        static Scope<IndexBuffer> Create(const std::vector<uint32_t>& indices);
        static Scope<IndexBuffer> Create(const std::vector<uint16_t>& indices);

        // Bind to command buffer
        void Bind(VkCommandBuffer cmd) const;

        // Accessors
        VkBuffer GetVkBuffer() const { return m_Buffer->GetVkBuffer(); }
        uint32_t GetCount() const { return m_Count; }
        VkIndexType GetIndexType() const { return m_IndexType; }

    private:
        Scope<Buffer> m_Buffer;
        uint32_t m_Count = 0;
        VkIndexType m_IndexType = VK_INDEX_TYPE_UINT32;
    };

}
