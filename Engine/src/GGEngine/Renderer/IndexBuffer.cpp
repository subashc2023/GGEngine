#include "ggpch.h"
#include "IndexBuffer.h"
#include "Platform/Vulkan/VulkanRHI.h"
#include <vulkan/vulkan.h>

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
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer buffer = registry.GetBuffer(m_Buffer->GetHandle());
        VkIndexType vkIndexType = ToVulkan(m_IndexType);

        vkCmdBindIndexBuffer(vkCmd, buffer, 0, vkIndexType);
    }

    void IndexBuffer::BindVk(void* vkCmd) const
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkBuffer buffer = registry.GetBuffer(m_Buffer->GetHandle());
        VkIndexType vkIndexType = ToVulkan(m_IndexType);

        vkCmdBindIndexBuffer(static_cast<VkCommandBuffer>(vkCmd), buffer, 0, vkIndexType);
    }

}
