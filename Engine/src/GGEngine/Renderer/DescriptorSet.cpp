#include "ggpch.h"
#include "DescriptorSet.h"
#include "UniformBuffer.h"
#include "GGEngine/Asset/Texture.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Core/Log.h"

namespace GGEngine {

    // DescriptorSetLayout implementation
    DescriptorSetLayout::DescriptorSetLayout(const std::vector<DescriptorBinding>& bindings)
        : m_Bindings(bindings)
    {
        auto device = VulkanContext::Get().GetDevice();

        std::vector<VkDescriptorSetLayoutBinding> vkBindings;
        vkBindings.reserve(bindings.size());

        for (const auto& binding : bindings)
        {
            VkDescriptorSetLayoutBinding vkBinding{};
            vkBinding.binding = binding.binding;
            vkBinding.descriptorType = binding.type;
            vkBinding.descriptorCount = binding.count;
            vkBinding.stageFlags = binding.stageFlags;
            vkBinding.pImmutableSamplers = nullptr;
            vkBindings.push_back(vkBinding);
        }

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.bindingCount = static_cast<uint32_t>(vkBindings.size());
        layoutInfo.pBindings = vkBindings.data();

        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &m_Layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create descriptor set layout!");
        }
    }

    DescriptorSetLayout::~DescriptorSetLayout()
    {
        if (m_Layout != VK_NULL_HANDLE)
        {
            vkDestroyDescriptorSetLayout(VulkanContext::Get().GetDevice(), m_Layout, nullptr);
        }
    }

    // DescriptorSet implementation
    DescriptorSet::DescriptorSet(const DescriptorSetLayout& layout)
        : m_Layout(&layout)
    {
        auto& vkContext = VulkanContext::Get();
        auto device = vkContext.GetDevice();
        auto pool = vkContext.GetDescriptorPool();

        VkDescriptorSetLayout vkLayout = layout.GetVkLayout();

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &vkLayout;

        if (vkAllocateDescriptorSets(device, &allocInfo, &m_DescriptorSet) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to allocate descriptor set!");
        }
    }

    DescriptorSet::~DescriptorSet()
    {
        if (m_DescriptorSet != VK_NULL_HANDLE)
        {
            auto& vkContext = VulkanContext::Get();
            vkFreeDescriptorSets(vkContext.GetDevice(), vkContext.GetDescriptorPool(), 1, &m_DescriptorSet);
        }
    }

    void DescriptorSet::SetUniformBuffer(uint32_t binding, const UniformBuffer& buffer)
    {
        auto device = VulkanContext::Get().GetDevice();

        VkDescriptorBufferInfo bufferInfo = buffer.GetDescriptorInfo();

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = m_DescriptorSet;
        write.dstBinding = binding;
        write.dstArrayElement = 0;
        write.descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        write.descriptorCount = 1;
        write.pBufferInfo = &bufferInfo;

        vkUpdateDescriptorSets(device, 1, &write, 0, nullptr);
    }

    void DescriptorSet::SetTexture(uint32_t binding, const Texture& texture)
    {
        auto device = VulkanContext::Get().GetDevice();

        VkDescriptorImageInfo imageInfo = texture.GetDescriptorInfo();

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = m_DescriptorSet;
        write.dstBinding = binding;
        write.dstArrayElement = 0;
        write.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        write.descriptorCount = 1;
        write.pImageInfo = &imageInfo;

        vkUpdateDescriptorSets(device, 1, &write, 0, nullptr);
    }

    void DescriptorSet::Bind(VkCommandBuffer cmd, VkPipelineLayout pipelineLayout, uint32_t setIndex) const
    {
        vkCmdBindDescriptorSets(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipelineLayout,
                                setIndex, 1, &m_DescriptorSet, 0, nullptr);
    }

}
