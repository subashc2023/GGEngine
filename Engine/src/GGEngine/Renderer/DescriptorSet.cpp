#include "ggpch.h"
#include "DescriptorSet.h"
#include "UniformBuffer.h"
#include "GGEngine/Asset/Texture.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "Platform/Vulkan/VulkanRHI.h"
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
            vkBinding.descriptorType = ToVulkan(binding.type);
            vkBinding.descriptorCount = binding.count;
            vkBinding.stageFlags = ToVulkan(binding.stageFlags);
            vkBinding.pImmutableSamplers = nullptr;
            vkBindings.push_back(vkBinding);
        }

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.bindingCount = static_cast<uint32_t>(vkBindings.size());
        layoutInfo.pBindings = vkBindings.data();

        VkDescriptorSetLayout layout = VK_NULL_HANDLE;
        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create descriptor set layout!");
            return;
        }

        // Register with resource registry
        m_Handle = VulkanResourceRegistry::Get().RegisterDescriptorSetLayout(layout);
    }

    DescriptorSetLayout::~DescriptorSetLayout()
    {
        if (m_Handle.IsValid())
        {
            auto& registry = VulkanResourceRegistry::Get();
            VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(m_Handle);
            if (layout != VK_NULL_HANDLE)
            {
                vkDestroyDescriptorSetLayout(VulkanContext::Get().GetDevice(), layout, nullptr);
            }
            registry.UnregisterDescriptorSetLayout(m_Handle);
        }
    }

    // DescriptorSet implementation
    DescriptorSet::DescriptorSet(const DescriptorSetLayout& layout)
        : m_Layout(&layout)
    {
        auto& vkContext = VulkanContext::Get();
        auto device = vkContext.GetDevice();
        auto pool = vkContext.GetDescriptorPool();

        VkDescriptorSetLayout vkLayout = VulkanResourceRegistry::Get().GetDescriptorSetLayout(layout.GetHandle());

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &vkLayout;

        VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
        if (vkAllocateDescriptorSets(device, &allocInfo, &descriptorSet) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to allocate descriptor set!");
            return;
        }

        // Register with resource registry
        m_Handle = VulkanResourceRegistry::Get().RegisterDescriptorSet(descriptorSet, layout.GetHandle());
    }

    DescriptorSet::~DescriptorSet()
    {
        if (m_Handle.IsValid())
        {
            auto& registry = VulkanResourceRegistry::Get();
            VkDescriptorSet descriptorSet = registry.GetDescriptorSet(m_Handle);
            if (descriptorSet != VK_NULL_HANDLE)
            {
                auto& vkContext = VulkanContext::Get();
                vkFreeDescriptorSets(vkContext.GetDevice(), vkContext.GetDescriptorPool(), 1, &descriptorSet);
            }
            registry.UnregisterDescriptorSet(m_Handle);
        }
    }

    void DescriptorSet::SetUniformBuffer(uint32_t binding, const UniformBuffer& buffer)
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        // Build descriptor buffer info from handle
        VkDescriptorBufferInfo bufferInfo{};
        bufferInfo.buffer = registry.GetBuffer(buffer.GetHandle());
        bufferInfo.offset = 0;
        bufferInfo.range = buffer.GetSize();

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = registry.GetDescriptorSet(m_Handle);
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
        auto& registry = VulkanResourceRegistry::Get();

        // Build descriptor image info from texture handle
        auto textureData = registry.GetTextureData(texture.GetHandle());
        VkDescriptorImageInfo imageInfo{};
        imageInfo.sampler = textureData.sampler;
        imageInfo.imageView = textureData.imageView;
        imageInfo.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = registry.GetDescriptorSet(m_Handle);
        write.dstBinding = binding;
        write.dstArrayElement = 0;
        write.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        write.descriptorCount = 1;
        write.pImageInfo = &imageInfo;

        vkUpdateDescriptorSets(device, 1, &write, 0, nullptr);
    }

    void DescriptorSet::Bind(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle pipelineLayout, uint32_t setIndex) const
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout vkLayout = registry.GetPipelineLayout(pipelineLayout);
        VkDescriptorSet vkSet = registry.GetDescriptorSet(m_Handle);

        vkCmdBindDescriptorSets(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, vkLayout,
                                setIndex, 1, &vkSet, 0, nullptr);
    }

    void DescriptorSet::BindVk(void* vkCmd, void* vkPipelineLayout, uint32_t setIndex) const
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSet vkSet = registry.GetDescriptorSet(m_Handle);

        vkCmdBindDescriptorSets(static_cast<VkCommandBuffer>(vkCmd), VK_PIPELINE_BIND_POINT_GRAPHICS,
                                static_cast<VkPipelineLayout>(vkPipelineLayout),
                                setIndex, 1, &vkSet, 0, nullptr);
    }

}
