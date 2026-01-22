#include "ggpch.h"
#include "DescriptorSet.h"
#include "UniformBuffer.h"
#include "GGEngine/Asset/Texture.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/RHI/RHICommandBuffer.h"
#include "GGEngine/Core/Log.h"

namespace GGEngine {

    // DescriptorSetLayout implementation
    DescriptorSetLayout::DescriptorSetLayout(const std::vector<DescriptorBinding>& bindings)
        : m_Bindings(bindings)
    {
        // Convert to RHI bindings
        std::vector<RHIDescriptorBinding> rhiBindings;
        rhiBindings.reserve(bindings.size());
        for (const auto& binding : bindings)
        {
            RHIDescriptorBinding rhiBinding;
            rhiBinding.binding = binding.binding;
            rhiBinding.type = binding.type;
            rhiBinding.stages = binding.stageFlags;
            rhiBinding.count = binding.count;
            rhiBindings.push_back(rhiBinding);
        }

        // Create layout through RHI device
        m_Handle = RHIDevice::Get().CreateDescriptorSetLayout(rhiBindings);
        if (!m_Handle.IsValid())
        {
            GG_CORE_ERROR("Failed to create descriptor set layout!");
        }
    }

    DescriptorSetLayout::~DescriptorSetLayout()
    {
        if (m_Handle.IsValid())
        {
            RHIDevice::Get().DestroyDescriptorSetLayout(m_Handle);
        }
    }

    // DescriptorSet implementation
    DescriptorSet::DescriptorSet(const DescriptorSetLayout& layout)
        : m_Layout(&layout)
    {
        // Allocate descriptor set through RHI device
        m_Handle = RHIDevice::Get().AllocateDescriptorSet(layout.GetHandle());
        if (!m_Handle.IsValid())
        {
            GG_CORE_ERROR("Failed to allocate descriptor set!");
        }
    }

    DescriptorSet::~DescriptorSet()
    {
        if (m_Handle.IsValid())
        {
            RHIDevice::Get().FreeDescriptorSet(m_Handle);
        }
    }

    void DescriptorSet::SetUniformBuffer(uint32_t binding, const UniformBuffer& buffer)
    {
        std::vector<RHIDescriptorWrite> writes;
        writes.push_back(RHIDescriptorWrite::UniformBuffer(binding, buffer.GetHandle(), 0, buffer.GetSize()));
        RHIDevice::Get().UpdateDescriptorSet(m_Handle, writes);
    }

    void DescriptorSet::SetTexture(uint32_t binding, const Texture& texture)
    {
        SetTextureAtIndex(binding, 0, texture);
    }

    void DescriptorSet::SetTextureAtIndex(uint32_t binding, uint32_t arrayIndex, const Texture& texture)
    {
        // Get sampler from texture's separate handle or fallback to null sampler
        RHISamplerHandle samplerHandle = texture.GetSamplerHandle();

        RHIDescriptorWrite write;
        write.binding = binding;
        write.arrayElement = arrayIndex;
        write.type = DescriptorType::CombinedImageSampler;
        write.resource = RHIDescriptorImageInfo{ samplerHandle, texture.GetHandle(), ImageLayout::ShaderReadOnly };

        std::vector<RHIDescriptorWrite> writes = { write };
        RHIDevice::Get().UpdateDescriptorSet(m_Handle, writes);
    }

    void DescriptorSet::Bind(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle pipelineLayout, uint32_t setIndex) const
    {
        RHICmd::BindDescriptorSet(cmd, pipelineLayout, m_Handle, setIndex);
    }

}
