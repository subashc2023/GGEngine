#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"
#include "Buffer.h"
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

        // Bind to command buffer (RHI handle)
        void Bind(RHICommandBufferHandle cmd) const;

        // Bind to command buffer (Vulkan - for backward compatibility during migration)
        void BindVk(void* vkCmd) const;

        // Accessors
        RHIBufferHandle GetHandle() const { return m_Buffer->GetHandle(); }
        uint32_t GetCount() const { return m_Count; }
        IndexType GetIndexType() const { return m_IndexType; }

    private:
        Scope<Buffer> m_Buffer;
        uint32_t m_Count = 0;
        IndexType m_IndexType = IndexType::UInt32;
    };

}
