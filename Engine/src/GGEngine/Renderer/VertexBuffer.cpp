#include "ggpch.h"
#include "VertexBuffer.h"

namespace GGEngine {

    VertexBuffer::VertexBuffer(const void* vertices, uint64_t size, const VertexLayout& layout)
        : m_Layout(layout)
    {
        BufferSpecification spec;
        spec.size = size;
        spec.usage = BufferUsage::Vertex;
        spec.cpuVisible = false;

        m_Buffer = CreateScope<Buffer>(spec);

        if (vertices)
        {
            m_Buffer->SetData(vertices, size);
        }
    }

    VertexBuffer::VertexBuffer(uint64_t size, const VertexLayout& layout)
        : m_Layout(layout)
    {
        BufferSpecification spec;
        spec.size = size;
        spec.usage = BufferUsage::Vertex;
        spec.cpuVisible = false;  // GPU-only memory for better performance

        m_Buffer = CreateScope<Buffer>(spec);
    }

    Scope<VertexBuffer> VertexBuffer::Create(const void* data, uint64_t size, const VertexLayout& layout)
    {
        return CreateScope<VertexBuffer>(data, size, layout);
    }

    void VertexBuffer::Bind(VkCommandBuffer cmd, uint32_t binding) const
    {
        VkBuffer buffers[] = { m_Buffer->GetVkBuffer() };
        VkDeviceSize offsets[] = { 0 };
        vkCmdBindVertexBuffers(cmd, binding, 1, buffers, offsets);
    }

    void VertexBuffer::SetData(const void* data, uint64_t size, uint64_t offset)
    {
        m_Buffer->SetData(data, size, offset);
    }

    uint32_t VertexBuffer::GetVertexCount() const
    {
        if (m_Layout.GetStride() == 0)
            return 0;
        return static_cast<uint32_t>(m_Buffer->GetSize() / m_Layout.GetStride());
    }

}
