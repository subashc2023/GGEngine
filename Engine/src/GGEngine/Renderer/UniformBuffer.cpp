#include "ggpch.h"
#include "UniformBuffer.h"

namespace GGEngine {

    UniformBuffer::UniformBuffer(uint64_t size)
    {
        BufferSpecification spec;
        spec.size = size;
        spec.usage = BufferUsage::Uniform;
        spec.cpuVisible = true;  // Uniforms are updated frequently from CPU

        m_Buffer = std::make_unique<Buffer>(spec);
    }

    void UniformBuffer::SetData(const void* data, uint64_t size, uint64_t offset)
    {
        m_Buffer->SetData(data, size, offset);
    }

    VkDescriptorBufferInfo UniformBuffer::GetDescriptorInfo() const
    {
        VkDescriptorBufferInfo info{};
        info.buffer = m_Buffer->GetVkBuffer();
        info.offset = 0;
        info.range = m_Buffer->GetSize();
        return info;
    }

}
