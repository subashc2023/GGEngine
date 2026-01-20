#pragma once

#include "GGEngine/Core.h"
#include "Buffer.h"
#include "VertexLayout.h"
#include <vulkan/vulkan.h>
#include <memory>
#include <vector>

namespace GGEngine {

    class GG_API VertexBuffer
    {
    public:
        // Create from raw data
        VertexBuffer(const void* vertices, uint64_t size, const VertexLayout& layout);

        // Create with size only (for dynamic updates)
        VertexBuffer(uint64_t size, const VertexLayout& layout);

        ~VertexBuffer() = default;

        VertexBuffer(const VertexBuffer&) = delete;
        VertexBuffer& operator=(const VertexBuffer&) = delete;

        // Factory methods
        template<typename T>
        static std::unique_ptr<VertexBuffer> Create(const std::vector<T>& vertices, const VertexLayout& layout)
        {
            return std::make_unique<VertexBuffer>(vertices.data(), vertices.size() * sizeof(T), layout);
        }

        static std::unique_ptr<VertexBuffer> Create(const void* data, uint64_t size, const VertexLayout& layout);

        // Bind to command buffer
        void Bind(VkCommandBuffer cmd, uint32_t binding = 0) const;

        // Update data
        void SetData(const void* data, uint64_t size, uint64_t offset = 0);

        // Accessors
        const VertexLayout& GetLayout() const { return m_Layout; }
        VkBuffer GetVkBuffer() const { return m_Buffer->GetVkBuffer(); }
        uint64_t GetSize() const { return m_Buffer->GetSize(); }
        uint32_t GetVertexCount() const;

    private:
        std::unique_ptr<Buffer> m_Buffer;
        VertexLayout m_Layout;
    };

}
