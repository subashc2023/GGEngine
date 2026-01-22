#include "ggpch.h"
#include "BindlessTextureManager.h"
#include "GGEngine/Asset/Texture.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "Platform/Vulkan/VulkanRHI.h"

#include <vulkan/vulkan.h>

namespace GGEngine {

    BindlessTextureManager& BindlessTextureManager::Get()
    {
        static BindlessTextureManager instance;
        return instance;
    }

    void BindlessTextureManager::Init(uint32_t maxTextures)
    {
        if (m_Initialized)
        {
            GG_CORE_WARN("BindlessTextureManager already initialized!");
            return;
        }

        auto& ctx = VulkanContext::Get();
        VkDevice device = ctx.GetDevice();

        // Clamp maxTextures to device limits
        const auto& limits = ctx.GetBindlessLimits();
        uint32_t effectiveMax = std::min(maxTextures, limits.maxPerStageDescriptorSampledImages);
        if (effectiveMax < maxTextures)
        {
            GG_CORE_WARN("Requested {} bindless textures, but device only supports {}",
                         maxTextures, effectiveMax);
        }
        m_MaxTextures = effectiveMax;

        // Create descriptor pool with UPDATE_AFTER_BIND flag
        VkDescriptorPoolSize poolSize{};
        poolSize.type = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        poolSize.descriptorCount = m_MaxTextures;

        VkDescriptorPoolCreateInfo poolInfo{};
        poolInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO;
        poolInfo.flags = VK_DESCRIPTOR_POOL_CREATE_UPDATE_AFTER_BIND_BIT;
        poolInfo.maxSets = 1;  // Single global descriptor set
        poolInfo.poolSizeCount = 1;
        poolInfo.pPoolSizes = &poolSize;

        VkDescriptorPool pool = VK_NULL_HANDLE;
        if (vkCreateDescriptorPool(device, &poolInfo, nullptr, &pool) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create bindless descriptor pool!");
            return;
        }
        m_DescriptorPool = pool;

        // Create descriptor set layout with PARTIALLY_BOUND and UPDATE_AFTER_BIND flags
        VkDescriptorSetLayoutBinding binding{};
        binding.binding = 0;
        binding.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        binding.descriptorCount = m_MaxTextures;
        binding.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
        binding.pImmutableSamplers = nullptr;

        // Binding flags for bindless: partially bound + update after bind
        VkDescriptorBindingFlags bindingFlags =
            VK_DESCRIPTOR_BINDING_PARTIALLY_BOUND_BIT |
            VK_DESCRIPTOR_BINDING_UPDATE_AFTER_BIND_BIT |
            VK_DESCRIPTOR_BINDING_VARIABLE_DESCRIPTOR_COUNT_BIT;

        VkDescriptorSetLayoutBindingFlagsCreateInfo bindingFlagsInfo{};
        bindingFlagsInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_BINDING_FLAGS_CREATE_INFO;
        bindingFlagsInfo.bindingCount = 1;
        bindingFlagsInfo.pBindingFlags = &bindingFlags;

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.pNext = &bindingFlagsInfo;
        layoutInfo.flags = VK_DESCRIPTOR_SET_LAYOUT_CREATE_UPDATE_AFTER_BIND_POOL_BIT;
        layoutInfo.bindingCount = 1;
        layoutInfo.pBindings = &binding;

        VkDescriptorSetLayout layout = VK_NULL_HANDLE;
        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create bindless descriptor set layout!");
            vkDestroyDescriptorPool(device, pool, nullptr);
            m_DescriptorPool = nullptr;
            return;
        }
        m_DescriptorSetLayout = layout;

        // Register layout with RHI registry
        m_LayoutHandle = VulkanResourceRegistry::Get().RegisterDescriptorSetLayout(layout);

        // Allocate descriptor set with variable descriptor count
        VkDescriptorSetVariableDescriptorCountAllocateInfo variableCountInfo{};
        variableCountInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_VARIABLE_DESCRIPTOR_COUNT_ALLOCATE_INFO;
        variableCountInfo.descriptorSetCount = 1;
        variableCountInfo.pDescriptorCounts = &m_MaxTextures;

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.pNext = &variableCountInfo;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &layout;

        VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
        if (vkAllocateDescriptorSets(device, &allocInfo, &descriptorSet) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to allocate bindless descriptor set!");
            vkDestroyDescriptorSetLayout(device, layout, nullptr);
            vkDestroyDescriptorPool(device, pool, nullptr);
            m_DescriptorSetLayout = nullptr;
            m_DescriptorPool = nullptr;
            return;
        }
        m_DescriptorSet = descriptorSet;

        // Initialize slot 0 with white/fallback texture
        // This will be done when the fallback texture registers itself
        m_NextIndex = 0;

        m_Initialized = true;
        GG_CORE_INFO("BindlessTextureManager initialized: max {} textures", m_MaxTextures);
    }

    void BindlessTextureManager::Shutdown()
    {
        if (!m_Initialized)
            return;

        auto& ctx = VulkanContext::Get();
        VkDevice device = ctx.GetDevice();

        if (device == VK_NULL_HANDLE)
        {
            m_Initialized = false;
            return;
        }

        // Descriptor set is implicitly freed when pool is destroyed

        if (m_DescriptorSetLayout)
        {
            VulkanResourceRegistry::Get().UnregisterDescriptorSetLayout(m_LayoutHandle);
            vkDestroyDescriptorSetLayout(device, static_cast<VkDescriptorSetLayout>(m_DescriptorSetLayout), nullptr);
            m_DescriptorSetLayout = nullptr;
        }

        if (m_DescriptorPool)
        {
            vkDestroyDescriptorPool(device, static_cast<VkDescriptorPool>(m_DescriptorPool), nullptr);
            m_DescriptorPool = nullptr;
        }

        m_HandleToIndex.clear();
        while (!m_FreeIndices.empty())
            m_FreeIndices.pop();

        m_TextureCount = 0;
        m_NextIndex = 0;
        m_Initialized = false;

        GG_CORE_TRACE("BindlessTextureManager shutdown");
    }

    BindlessTextureIndex BindlessTextureManager::RegisterTexture(const Texture& texture)
    {
        if (!m_Initialized)
        {
            GG_CORE_WARN("BindlessTextureManager not initialized!");
            return InvalidBindlessIndex;
        }

        RHITextureHandle handle = texture.GetHandle();
        if (!handle.IsValid())
        {
            GG_CORE_WARN("Cannot register texture with invalid handle!");
            return InvalidBindlessIndex;
        }

        // Check if already registered
        auto it = m_HandleToIndex.find(handle.id);
        if (it != m_HandleToIndex.end())
        {
            return it->second;
        }

        // Get index from free list or allocate new
        BindlessTextureIndex index;
        if (!m_FreeIndices.empty())
        {
            index = m_FreeIndices.front();
            m_FreeIndices.pop();
        }
        else
        {
            if (m_NextIndex >= m_MaxTextures)
            {
                GG_CORE_ERROR("BindlessTextureManager: Maximum texture count ({}) exceeded!", m_MaxTextures);
                return InvalidBindlessIndex;
            }
            index = m_NextIndex++;
        }

        // Get texture data from registry
        auto& registry = VulkanResourceRegistry::Get();
        auto texData = registry.GetTextureData(handle);

        // Update descriptor
        VkDescriptorImageInfo imageInfo{};
        imageInfo.sampler = texData.sampler;
        imageInfo.imageView = texData.imageView;
        imageInfo.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = static_cast<VkDescriptorSet>(m_DescriptorSet);
        write.dstBinding = 0;
        write.dstArrayElement = index;
        write.descriptorCount = 1;
        write.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        write.pImageInfo = &imageInfo;

        vkUpdateDescriptorSets(VulkanContext::Get().GetDevice(), 1, &write, 0, nullptr);

        // Store mapping
        m_HandleToIndex[handle.id] = index;
        m_TextureCount++;

        return index;
    }

    void BindlessTextureManager::UnregisterTexture(BindlessTextureIndex index)
    {
        if (!m_Initialized)
            return;

        if (index == InvalidBindlessIndex || index >= m_NextIndex)
            return;

        // Find and remove from handle map
        for (auto it = m_HandleToIndex.begin(); it != m_HandleToIndex.end(); ++it)
        {
            if (it->second == index)
            {
                m_HandleToIndex.erase(it);
                break;
            }
        }

        // Add to free list for reuse
        m_FreeIndices.push(index);
        m_TextureCount--;

        // Note: We don't clear the descriptor slot - it's marked as "partially bound"
        // so invalid/unused slots are allowed. The slot will be overwritten when reused.
    }

    void* BindlessTextureManager::GetDescriptorSet() const
    {
        return m_DescriptorSet;
    }

    void* BindlessTextureManager::GetDescriptorSetLayout() const
    {
        return m_DescriptorSetLayout;
    }

}
