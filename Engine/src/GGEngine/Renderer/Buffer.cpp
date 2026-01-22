#include "ggpch.h"
#include "Buffer.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/Core/Log.h"
#include <cstring>

namespace GGEngine {

    Buffer::Buffer(const BufferSpecification& spec)
        : m_Specification(spec)
    {
        Create();
    }

    Buffer::~Buffer()
    {
        Destroy();
    }

    void Buffer::Create()
    {
        if (m_Specification.size == 0)
        {
            GG_CORE_ERROR("Buffer creation failed: size is 0");
            return;
        }

        // Convert to RHI specification
        RHIBufferSpecification rhiSpec;
        rhiSpec.size = m_Specification.size;
        rhiSpec.usage = m_Specification.usage;
        rhiSpec.cpuVisible = m_Specification.cpuVisible;
        rhiSpec.debugName = m_Specification.debugName;

        // Create buffer through RHI device
        m_Handle = RHIDevice::Get().CreateBuffer(rhiSpec);

        if (!m_Handle.IsValid())
        {
            GG_CORE_ERROR("Failed to create buffer through RHI!");
            return;
        }

        if (!m_Specification.debugName.empty())
        {
            GG_CORE_TRACE("Buffer '{}' created ({} bytes)", m_Specification.debugName, m_Specification.size);
        }
    }

    void Buffer::Destroy()
    {
        if (m_Handle.IsValid())
        {
            RHIDevice::Get().DestroyBuffer(m_Handle);
            m_Handle = NullBuffer;
        }
    }

    void Buffer::SetData(const void* data, uint64_t size, uint64_t offset)
    {
        if (size == 0 || data == nullptr)
            return;

        if (offset + size > m_Specification.size)
        {
            GG_CORE_ERROR("Buffer::SetData: data exceeds buffer size");
            return;
        }

        // Use RHI device for data upload (handles staging internally for non-CPU-visible buffers)
        RHIDevice::Get().UploadBufferData(m_Handle, data, size, offset);
    }

}
