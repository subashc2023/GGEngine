#include "ggpch.h"
#include "BindlessTextureManager.h"
#include "GGEngine/Asset/Texture.h"
#include "GGEngine/RHI/RHIDevice.h"

namespace GGEngine {

    BindlessTextureManager& BindlessTextureManager::Get()
    {
        static BindlessTextureManager instance;
        return instance;
    }

    void BindlessTextureManager::Init(uint32_t maxTextures, Filter minFilter, Filter magFilter)
    {
        if (m_Initialized)
        {
            GG_CORE_WARN("BindlessTextureManager already initialized!");
            return;
        }

        auto& device = RHIDevice::Get();

        // Store filter settings
        m_MinFilter = minFilter;
        m_MagFilter = magFilter;

        // Clamp maxTextures to device limits
        uint32_t deviceMax = device.GetMaxBindlessTextures();
        uint32_t effectiveMax = std::min(maxTextures, deviceMax);
        if (effectiveMax < maxTextures)
        {
            GG_CORE_WARN("Requested {} bindless textures, but device only supports {}",
                         maxTextures, effectiveMax);
        }
        m_MaxTextures = effectiveMax;

        // Create shared sampler with configured filtering
        RHISamplerSpecification samplerSpec;
        samplerSpec.magFilter = m_MagFilter;
        samplerSpec.minFilter = m_MinFilter;
        samplerSpec.mipmapMode = (m_MinFilter == Filter::Nearest) ? MipmapMode::Nearest : MipmapMode::Linear;
        samplerSpec.addressModeU = AddressMode::Repeat;
        samplerSpec.addressModeV = AddressMode::Repeat;
        samplerSpec.addressModeW = AddressMode::Repeat;
        samplerSpec.mipLodBias = 0.0f;
        samplerSpec.anisotropyEnable = false;
        samplerSpec.maxAnisotropy = 1.0f;
        samplerSpec.compareEnable = false;
        samplerSpec.compareOp = CompareOp::Always;
        samplerSpec.minLod = 0.0f;
        samplerSpec.maxLod = 1000.0f;
        samplerSpec.borderColor = BorderColor::IntOpaqueBlack;

        m_SharedSampler = device.CreateSampler(samplerSpec);
        if (!m_SharedSampler.IsValid())
        {
            GG_CORE_ERROR("BindlessTextureManager: Failed to create shared sampler!");
            return;
        }

        // Create descriptor set layout with immutable sampler at binding 0, texture array at binding 1
        m_LayoutHandle = device.CreateBindlessSamplerTextureLayout(m_SharedSampler, m_MaxTextures);
        if (!m_LayoutHandle.IsValid())
        {
            device.DestroySampler(m_SharedSampler);
            m_SharedSampler = NullSampler;
            GG_CORE_ERROR("BindlessTextureManager: Failed to create descriptor set layout!");
            return;
        }

        // Allocate descriptor set
        m_DescriptorSetHandle = device.AllocateBindlessSamplerTextureSet(m_LayoutHandle, m_MaxTextures);
        if (!m_DescriptorSetHandle.IsValid())
        {
            device.DestroyDescriptorSetLayout(m_LayoutHandle);
            m_LayoutHandle = NullDescriptorSetLayout;
            device.DestroySampler(m_SharedSampler);
            m_SharedSampler = NullSampler;
            GG_CORE_ERROR("BindlessTextureManager: Failed to allocate descriptor set!");
            return;
        }

        m_NextIndex = 0;
        m_Initialized = true;
        GG_CORE_INFO("BindlessTextureManager initialized: max {} textures (separate sampler mode)", m_MaxTextures);
    }

    void BindlessTextureManager::Shutdown()
    {
        if (!m_Initialized)
            return;

        auto& device = RHIDevice::Get();

        // Free descriptor set (pool is implicitly destroyed)
        if (m_DescriptorSetHandle.IsValid())
        {
            device.FreeDescriptorSet(m_DescriptorSetHandle);
            m_DescriptorSetHandle = NullDescriptorSet;
        }

        // Destroy layout
        if (m_LayoutHandle.IsValid())
        {
            device.DestroyDescriptorSetLayout(m_LayoutHandle);
            m_LayoutHandle = NullDescriptorSetLayout;
        }

        // Destroy sampler
        if (m_SharedSampler.IsValid())
        {
            device.DestroySampler(m_SharedSampler);
            m_SharedSampler = NullSampler;
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
        std::lock_guard<std::mutex> lock(m_Mutex);

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

        // Update descriptor set with texture
        RHIDevice::Get().UpdateBindlessSamplerTextureSlot(m_DescriptorSetHandle, index, handle);

        // Store mapping
        m_HandleToIndex[handle.id] = index;
        m_TextureCount++;

        return index;
    }

    BindlessTextureIndex BindlessTextureManager::RegisterTextureAtIndex(const Texture& texture, BindlessTextureIndex index)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

        if (!m_Initialized)
        {
            GG_CORE_WARN("BindlessTextureManager not initialized!");
            return InvalidBindlessIndex;
        }

        if (index == InvalidBindlessIndex || index >= m_MaxTextures)
        {
            GG_CORE_WARN("RegisterTextureAtIndex: invalid index {}", index);
            return InvalidBindlessIndex;
        }

        RHITextureHandle handle = texture.GetHandle();
        if (!handle.IsValid())
        {
            GG_CORE_WARN("Cannot register texture with invalid handle!");
            return InvalidBindlessIndex;
        }

        // Update descriptor set at the specific index
        RHIDevice::Get().UpdateBindlessSamplerTextureSlot(m_DescriptorSetHandle, index, handle);

        // Update mapping (remove old handle if exists, add new)
        for (auto it = m_HandleToIndex.begin(); it != m_HandleToIndex.end(); )
        {
            if (it->second == index)
            {
                it = m_HandleToIndex.erase(it);
            }
            else
            {
                ++it;
            }
        }
        m_HandleToIndex[handle.id] = index;

        // Ensure NextIndex is at least index+1
        if (index >= m_NextIndex)
        {
            m_NextIndex = index + 1;
        }

        GG_CORE_TRACE("BindlessTextureManager: registered texture at index {} (hot reload)", index);
        return index;
    }

    void BindlessTextureManager::UnregisterTexture(BindlessTextureIndex index)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

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
        return RHIDevice::Get().GetRawDescriptorSet(m_DescriptorSetHandle);
    }

    void* BindlessTextureManager::GetDescriptorSetLayout() const
    {
        // This returns the raw Vulkan handle for backward compatibility
        // The proper way is to use GetLayoutHandle() and RHI APIs
        return nullptr;  // No longer exposing raw Vulkan handles
    }

}
