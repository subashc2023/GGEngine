#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "Buffer.h"
#include "VertexLayout.h"
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
        static Scope<VertexBuffer> Create(const std::vector<T>& vertices, const VertexLayout& layout)
        {
            return CreateScope<VertexBuffer>(vertices.data(), vertices.size() * sizeof(T), layout);
        }

        static Scope<VertexBuffer> Create(const void* data, uint64_t size, const VertexLayout& layout);

        // Bind to command buffer (RHI handle)
        void Bind(RHICommandBufferHandle cmd, uint32_t binding = 0) const;

        // Update data
        void SetData(const void* data, uint64_t size, uint64_t offset = 0);

        // Accessors
        const VertexLayout& GetLayout() const { return m_Layout; }
        RHIBufferHandle GetHandle() const { return m_Buffer->GetHandle(); }
        uint64_t GetSize() const { return m_Buffer->GetSize(); }
        uint32_t GetVertexCount() const;

    private:
        Scope<Buffer> m_Buffer;
        VertexLayout m_Layout;
    };

}
