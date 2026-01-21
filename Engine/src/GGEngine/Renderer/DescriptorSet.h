#pragma once

#include "GGEngine/Core/Core.h"
#include <vulkan/vulkan.h>
#include <vector>
#include <memory>

namespace GGEngine {

    class UniformBuffer;

    // Descriptor binding specification
    struct GG_API DescriptorBinding
    {
        uint32_t binding = 0;
        VkDescriptorType type = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        VkShaderStageFlags stageFlags = VK_SHADER_STAGE_ALL_GRAPHICS;
        uint32_t count = 1;
    };

    // Descriptor Set Layout - describes the structure of bindings
    class GG_API DescriptorSetLayout
    {
    public:
        DescriptorSetLayout(const std::vector<DescriptorBinding>& bindings);
        ~DescriptorSetLayout();

        DescriptorSetLayout(const DescriptorSetLayout&) = delete;
        DescriptorSetLayout& operator=(const DescriptorSetLayout&) = delete;

        VkDescriptorSetLayout GetVkLayout() const { return m_Layout; }
        const std::vector<DescriptorBinding>& GetBindings() const { return m_Bindings; }

    private:
        VkDescriptorSetLayout m_Layout = VK_NULL_HANDLE;
        std::vector<DescriptorBinding> m_Bindings;
    };

    // Descriptor Set - allocated from pool, can be bound to pipeline
    class GG_API DescriptorSet
    {
    public:
        DescriptorSet(const DescriptorSetLayout& layout);
        ~DescriptorSet();

        DescriptorSet(const DescriptorSet&) = delete;
        DescriptorSet& operator=(const DescriptorSet&) = delete;

        // Update bindings
        void SetUniformBuffer(uint32_t binding, const UniformBuffer& buffer);
        // Future: SetTexture, SetStorageBuffer, etc.

        // Bind to command buffer
        void Bind(VkCommandBuffer cmd, VkPipelineLayout pipelineLayout, uint32_t setIndex = 0) const;

        VkDescriptorSet GetVkDescriptorSet() const { return m_DescriptorSet; }

    private:
        VkDescriptorSet m_DescriptorSet = VK_NULL_HANDLE;
        const DescriptorSetLayout* m_Layout;
    };

}
