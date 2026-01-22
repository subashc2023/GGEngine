#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"
#include <vector>
#include <memory>

namespace GGEngine {

    class UniformBuffer;
    class Texture;

    // Descriptor binding specification
    struct GG_API DescriptorBinding
    {
        uint32_t binding = 0;
        DescriptorType type = DescriptorType::UniformBuffer;
        ShaderStage stageFlags = ShaderStage::AllGraphics;
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

        RHIDescriptorSetLayoutHandle GetHandle() const { return m_Handle; }
        const std::vector<DescriptorBinding>& GetBindings() const { return m_Bindings; }

    private:
        RHIDescriptorSetLayoutHandle m_Handle;
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
        void SetTexture(uint32_t binding, const Texture& texture);
        void SetTextureAtIndex(uint32_t binding, uint32_t arrayIndex, const Texture& texture);

        // Bind to command buffer (RHI handles)
        void Bind(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle pipelineLayout, uint32_t setIndex = 0) const;

        // Bind to command buffer (Vulkan - backward compatibility during migration)
        void BindVk(void* vkCmd, void* vkPipelineLayout, uint32_t setIndex = 0) const;

        RHIDescriptorSetHandle GetHandle() const { return m_Handle; }

    private:
        RHIDescriptorSetHandle m_Handle;
        const DescriptorSetLayout* m_Layout;
    };

}
