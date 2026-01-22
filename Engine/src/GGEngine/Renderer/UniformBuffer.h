#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "Buffer.h"

namespace GGEngine {

    // Forward declaration for descriptor info
    struct RHIDescriptorBufferInfo;

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
        RHIBufferHandle GetHandle() const { return m_Buffer->GetHandle(); }
        uint64_t GetSize() const { return m_Buffer->GetSize(); }

    private:
        Scope<Buffer> m_Buffer;
    };

}
