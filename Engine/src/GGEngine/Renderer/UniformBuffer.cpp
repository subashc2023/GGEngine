#include "ggpch.h"
#include "UniformBuffer.h"

namespace GGEngine {

    UniformBuffer::UniformBuffer(uint64_t size)
    {
        BufferSpecification spec;
        spec.size = size;
        spec.usage = BufferUsage::Uniform;
        spec.cpuVisible = true;  // Uniforms are updated frequently from CPU

        m_Buffer = CreateScope<Buffer>(spec);
    }

    void UniformBuffer::SetData(const void* data, uint64_t size, uint64_t offset)
    {
        m_Buffer->SetData(data, size, offset);
    }

}
