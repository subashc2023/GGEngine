#include "ggpch.h"
#include "VulkanResourceRegistry.h"
#include "VulkanContext.h"
#include "GGEngine/RHI/RHIDevice.h"
#include <backends/imgui_impl_vulkan.h>

namespace GGEngine {

    // ============================================================================
    // Bindless Texture Support
    // ============================================================================

    RHIDescriptorSetLayoutHandle RHIDevice::CreateBindlessTextureLayout(uint32_t maxTextures)
    {
        auto device = VulkanContext::Get().GetDevice();

        VkDescriptorSetLayoutBinding binding{};
        binding.binding = 0;
        binding.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        binding.descriptorCount = maxTextures;
        binding.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;

        VkDescriptorBindingFlags bindingFlags =
            VK_DESCRIPTOR_BINDING_PARTIALLY_BOUND_BIT |
            VK_DESCRIPTOR_BINDING_UPDATE_AFTER_BIND_BIT |
            VK_DESCRIPTOR_BINDING_VARIABLE_DESCRIPTOR_COUNT_BIT;

        VkDescriptorSetLayoutBindingFlagsCreateInfo flagsInfo{};
        flagsInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_BINDING_FLAGS_CREATE_INFO;
        flagsInfo.bindingCount = 1;
        flagsInfo.pBindingFlags = &bindingFlags;

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.pNext = &flagsInfo;
        layoutInfo.flags = VK_DESCRIPTOR_SET_LAYOUT_CREATE_UPDATE_AFTER_BIND_POOL_BIT;
        layoutInfo.bindingCount = 1;
        layoutInfo.pBindings = &binding;

        VkDescriptorSetLayout layout = VK_NULL_HANDLE;
        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateBindlessTextureLayout: vkCreateDescriptorSetLayout failed");
            return NullDescriptorSetLayout;
        }

        return VulkanResourceRegistry::Get().RegisterDescriptorSetLayout(layout);
    }

    RHIDescriptorSetHandle RHIDevice::AllocateBindlessDescriptorSet(RHIDescriptorSetLayoutHandle layoutHandle, uint32_t maxTextures)
    {
        if (!layoutHandle.IsValid()) return NullDescriptorSet;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(layoutHandle);

        // Create a dedicated pool for bindless
        VkDescriptorPoolSize poolSize{};
        poolSize.type = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        poolSize.descriptorCount = maxTextures;

        VkDescriptorPoolCreateInfo poolInfo{};
        poolInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO;
        poolInfo.flags = VK_DESCRIPTOR_POOL_CREATE_UPDATE_AFTER_BIND_BIT;
        poolInfo.maxSets = 1;
        poolInfo.poolSizeCount = 1;
        poolInfo.pPoolSizes = &poolSize;

        VkDescriptorPool pool = VK_NULL_HANDLE;
        if (vkCreateDescriptorPool(device, &poolInfo, nullptr, &pool) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::AllocateBindlessDescriptorSet: vkCreateDescriptorPool failed");
            return NullDescriptorSet;
        }

        uint32_t variableCount = maxTextures;
        VkDescriptorSetVariableDescriptorCountAllocateInfo variableInfo{};
        variableInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_VARIABLE_DESCRIPTOR_COUNT_ALLOCATE_INFO;
        variableInfo.descriptorSetCount = 1;
        variableInfo.pDescriptorCounts = &variableCount;

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.pNext = &variableInfo;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &layout;

        VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
        if (vkAllocateDescriptorSets(device, &allocInfo, &descriptorSet) != VK_SUCCESS)
        {
            vkDestroyDescriptorPool(device, pool, nullptr);
            GG_CORE_ERROR("RHIDevice::AllocateBindlessDescriptorSet: vkAllocateDescriptorSets failed");
            return NullDescriptorSet;
        }

        // Register with the owning pool so it gets destroyed when the set is freed
        return registry.RegisterDescriptorSet(descriptorSet, layoutHandle, pool);
    }

    void RHIDevice::UpdateBindlessTexture(RHIDescriptorSetHandle setHandle, uint32_t index,
                                           RHITextureHandle textureHandle, RHISamplerHandle samplerHandle)
    {
        if (!setHandle.IsValid() || !textureHandle.IsValid()) return;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        VkDescriptorSet vkSet = registry.GetDescriptorSet(setHandle);
        auto textureData = registry.GetTextureData(textureHandle);
        VkSampler sampler = reinterpret_cast<VkSampler>(samplerHandle.id);

        VkDescriptorImageInfo imageInfo{};
        imageInfo.sampler = sampler;
        imageInfo.imageView = textureData.imageView;
        imageInfo.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = vkSet;
        write.dstBinding = 0;
        write.dstArrayElement = index;
        write.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        write.descriptorCount = 1;
        write.pImageInfo = &imageInfo;

        vkUpdateDescriptorSets(device, 1, &write, 0, nullptr);
    }

    uint32_t RHIDevice::GetMaxBindlessTextures() const
    {
        const auto& limits = VulkanContext::Get().GetBindlessLimits();
        return limits.maxPerStageDescriptorSampledImages;
    }

    void* RHIDevice::GetRawDescriptorSet(RHIDescriptorSetHandle handle) const
    {
        if (!handle.IsValid()) return nullptr;
        return VulkanResourceRegistry::Get().GetDescriptorSet(handle);
    }

    // ============================================================================
    // Bindless Texture Support (Separate Sampler Pattern)
    // ============================================================================

    RHIDescriptorSetLayoutHandle RHIDevice::CreateBindlessSamplerTextureLayout(
        RHISamplerHandle immutableSampler, uint32_t maxTextures)
    {
        auto device = VulkanContext::Get().GetDevice();
        VkSampler sampler = reinterpret_cast<VkSampler>(immutableSampler.id);

        // Two bindings: binding 0 = immutable sampler, binding 1 = texture array
        VkDescriptorSetLayoutBinding bindings[2] = {};

        // Binding 0: Immutable sampler
        bindings[0].binding = 0;
        bindings[0].descriptorType = VK_DESCRIPTOR_TYPE_SAMPLER;
        bindings[0].descriptorCount = 1;
        bindings[0].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
        bindings[0].pImmutableSamplers = &sampler;

        // Binding 1: Texture array (SAMPLED_IMAGE)
        bindings[1].binding = 1;
        bindings[1].descriptorType = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE;
        bindings[1].descriptorCount = maxTextures;
        bindings[1].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
        bindings[1].pImmutableSamplers = nullptr;

        // Binding flags: sampler has none, texture array needs bindless flags
        VkDescriptorBindingFlags bindingFlags[2] = {};
        bindingFlags[0] = 0;
        bindingFlags[1] = VK_DESCRIPTOR_BINDING_PARTIALLY_BOUND_BIT |
                         VK_DESCRIPTOR_BINDING_UPDATE_AFTER_BIND_BIT |
                         VK_DESCRIPTOR_BINDING_VARIABLE_DESCRIPTOR_COUNT_BIT;

        VkDescriptorSetLayoutBindingFlagsCreateInfo flagsInfo{};
        flagsInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_BINDING_FLAGS_CREATE_INFO;
        flagsInfo.bindingCount = 2;
        flagsInfo.pBindingFlags = bindingFlags;

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.pNext = &flagsInfo;
        layoutInfo.flags = VK_DESCRIPTOR_SET_LAYOUT_CREATE_UPDATE_AFTER_BIND_POOL_BIT;
        layoutInfo.bindingCount = 2;
        layoutInfo.pBindings = bindings;

        VkDescriptorSetLayout layout = VK_NULL_HANDLE;
        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateBindlessSamplerTextureLayout: vkCreateDescriptorSetLayout failed");
            return NullDescriptorSetLayout;
        }

        return VulkanResourceRegistry::Get().RegisterDescriptorSetLayout(layout);
    }

    RHIDescriptorSetHandle RHIDevice::AllocateBindlessSamplerTextureSet(
        RHIDescriptorSetLayoutHandle layoutHandle, uint32_t maxTextures)
    {
        if (!layoutHandle.IsValid()) return NullDescriptorSet;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(layoutHandle);

        // Create pool with UPDATE_AFTER_BIND for both sampler and sampled images
        VkDescriptorPoolSize poolSizes[2] = {};
        poolSizes[0].type = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE;
        poolSizes[0].descriptorCount = maxTextures;
        poolSizes[1].type = VK_DESCRIPTOR_TYPE_SAMPLER;
        poolSizes[1].descriptorCount = 1;

        VkDescriptorPoolCreateInfo poolInfo{};
        poolInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO;
        poolInfo.flags = VK_DESCRIPTOR_POOL_CREATE_UPDATE_AFTER_BIND_BIT;
        poolInfo.maxSets = 1;
        poolInfo.poolSizeCount = 2;
        poolInfo.pPoolSizes = poolSizes;

        VkDescriptorPool pool = VK_NULL_HANDLE;
        if (vkCreateDescriptorPool(device, &poolInfo, nullptr, &pool) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::AllocateBindlessSamplerTextureSet: vkCreateDescriptorPool failed");
            return NullDescriptorSet;
        }

        // Allocate with variable descriptor count (for texture array at binding 1)
        VkDescriptorSetVariableDescriptorCountAllocateInfo variableInfo{};
        variableInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_VARIABLE_DESCRIPTOR_COUNT_ALLOCATE_INFO;
        variableInfo.descriptorSetCount = 1;
        variableInfo.pDescriptorCounts = &maxTextures;

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.pNext = &variableInfo;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &layout;

        VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
        if (vkAllocateDescriptorSets(device, &allocInfo, &descriptorSet) != VK_SUCCESS)
        {
            vkDestroyDescriptorPool(device, pool, nullptr);
            GG_CORE_ERROR("RHIDevice::AllocateBindlessSamplerTextureSet: vkAllocateDescriptorSets failed");
            return NullDescriptorSet;
        }

        // Register with the owning pool so it gets destroyed when the set is freed
        return registry.RegisterDescriptorSet(descriptorSet, layoutHandle, pool);
    }

    void RHIDevice::UpdateBindlessSamplerTextureSlot(
        RHIDescriptorSetHandle setHandle, uint32_t index, RHITextureHandle textureHandle)
    {
        if (!setHandle.IsValid() || !textureHandle.IsValid()) return;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        VkDescriptorSet vkSet = registry.GetDescriptorSet(setHandle);
        auto textureData = registry.GetTextureData(textureHandle);

        VkDescriptorImageInfo imageInfo{};
        imageInfo.sampler = VK_NULL_HANDLE;  // Not used for SAMPLED_IMAGE
        imageInfo.imageView = textureData.imageView;
        imageInfo.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = vkSet;
        write.dstBinding = 1;  // Texture array is at binding 1
        write.dstArrayElement = index;
        write.descriptorType = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE;
        write.descriptorCount = 1;
        write.pImageInfo = &imageInfo;

        vkUpdateDescriptorSets(device, 1, &write, 0, nullptr);
    }

    // ============================================================================
    // ImGui Integration
    // ============================================================================

    void* RHIDevice::RegisterImGuiTexture(RHITextureHandle texture, RHISamplerHandle sampler)
    {
        if (!texture.IsValid() || !sampler.IsValid()) return nullptr;

        auto& registry = VulkanResourceRegistry::Get();
        auto textureData = registry.GetTextureData(texture);
        VkSampler vkSampler = reinterpret_cast<VkSampler>(sampler.id);

        return ImGui_ImplVulkan_AddTexture(
            vkSampler, textureData.imageView, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);
    }

    void RHIDevice::UnregisterImGuiTexture(void* imguiHandle)
    {
        if (imguiHandle)
        {
            ImGui_ImplVulkan_RemoveTexture(static_cast<VkDescriptorSet>(imguiHandle));
        }
    }

}
