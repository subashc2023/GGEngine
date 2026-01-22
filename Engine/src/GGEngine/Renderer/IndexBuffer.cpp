#include "ggpch.h"
#include "IndexBuffer.h"
#include "GGEngine/RHI/RHICommandBuffer.h"

namespace GGEngine {

    IndexBuffer::IndexBuffer(const uint32_t* indices, uint32_t count)
        : m_Count(count), m_IndexType(IndexType::UInt32)
    {
        BufferSpecification spec;
        spec.size = count * sizeof(uint32_t);
        spec.usage = BufferUsage::Index;
        spec.cpuVisible = false;

        m_Buffer = CreateScope<Buffer>(spec);
        m_Buffer->SetData(indices, spec.size);
    }

    IndexBuffer::IndexBuffer(const uint16_t* indices, uint32_t count)
        : m_Count(count), m_IndexType(IndexType::UInt16)
    {
        BufferSpecification spec;
        spec.size = count * sizeof(uint16_t);
        spec.usage = BufferUsage::Index;
        spec.cpuVisible = false;

        m_Buffer = CreateScope<Buffer>(spec);
        m_Buffer->SetData(indices, spec.size);
    }

    Scope<IndexBuffer> IndexBuffer::Create(const std::vector<uint32_t>& indices)
    {
        return CreateScope<IndexBuffer>(indices.data(), static_cast<uint32_t>(indices.size()));
    }

    Scope<IndexBuffer> IndexBuffer::Create(const std::vector<uint16_t>& indices)
    {
        return CreateScope<IndexBuffer>(indices.data(), static_cast<uint32_t>(indices.size()));
    }

    void IndexBuffer::Bind(RHICommandBufferHandle cmd) const
    {
        RHICmd::BindIndexBuffer(cmd, m_Buffer->GetHandle(), m_IndexType);
    }

}
