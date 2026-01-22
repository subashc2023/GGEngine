#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"
#include <cstdint>
#include <string>

namespace GGEngine {

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
        RHIBufferHandle GetHandle() const { return m_Handle; }
        uint64_t GetSize() const { return m_Specification.size; }
        const BufferSpecification& GetSpecification() const { return m_Specification; }

    protected:
        void Create();
        void Destroy();

        BufferSpecification m_Specification;
        RHIBufferHandle m_Handle;
    };

}
