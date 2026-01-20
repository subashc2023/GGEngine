#include "ggpch.h"
#include "IndexBuffer.h"

namespace GGEngine {

    IndexBuffer::IndexBuffer(const uint32_t* indices, uint32_t count)
        : m_Count(count), m_IndexType(VK_INDEX_TYPE_UINT32)
    {
        BufferSpecification spec;
        spec.size = count * sizeof(uint32_t);
        spec.usage = BufferUsage::Index;
        spec.cpuVisible = false;

        m_Buffer = std::make_unique<Buffer>(spec);
        m_Buffer->SetData(indices, spec.size);
    }

    IndexBuffer::IndexBuffer(const uint16_t* indices, uint32_t count)
        : m_Count(count), m_IndexType(VK_INDEX_TYPE_UINT16)
    {
        BufferSpecification spec;
        spec.size = count * sizeof(uint16_t);
        spec.usage = BufferUsage::Index;
        spec.cpuVisible = false;

        m_Buffer = std::make_unique<Buffer>(spec);
        m_Buffer->SetData(indices, spec.size);
    }

    std::unique_ptr<IndexBuffer> IndexBuffer::Create(const std::vector<uint32_t>& indices)
    {
        return std::make_unique<IndexBuffer>(indices.data(), static_cast<uint32_t>(indices.size()));
    }

    std::unique_ptr<IndexBuffer> IndexBuffer::Create(const std::vector<uint16_t>& indices)
    {
        return std::make_unique<IndexBuffer>(indices.data(), static_cast<uint32_t>(indices.size()));
    }

    void IndexBuffer::Bind(VkCommandBuffer cmd) const
    {
        vkCmdBindIndexBuffer(cmd, m_Buffer->GetVkBuffer(), 0, m_IndexType);
    }

}
