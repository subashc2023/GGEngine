#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"

#include <cstdint>
#include <queue>
#include <unordered_map>

namespace GGEngine {

    class Texture;

    // Index type for bindless textures
    using BindlessTextureIndex = uint32_t;
    constexpr BindlessTextureIndex InvalidBindlessIndex = UINT32_MAX;

    // Manages a global bindless descriptor set for all textures.
    // Uses Vulkan 1.2+ descriptor indexing features for UPDATE_AFTER_BIND
    // and PARTIALLY_BOUND descriptors.
    class GG_API BindlessTextureManager
    {
    public:
        static BindlessTextureManager& Get();

        // Initialize with maximum number of textures (default 16384)
        // Filter defaults to Nearest for pixel-art style rendering
        void Init(uint32_t maxTextures = 16384,
                  Filter minFilter = Filter::Nearest,
                  Filter magFilter = Filter::Nearest);
        void Shutdown();

        // Get the current sampler filter settings
        Filter GetMinFilter() const { return m_MinFilter; }
        Filter GetMagFilter() const { return m_MagFilter; }

        // Register a texture and get its bindless index
        BindlessTextureIndex RegisterTexture(const Texture& texture);

        // Unregister a texture, returning its index to the free list
        void UnregisterTexture(BindlessTextureIndex index);

        // Get the global bindless descriptor set (for binding in draw calls)
        void* GetDescriptorSet() const;

        // Get the descriptor set layout handle
        RHIDescriptorSetLayoutHandle GetLayoutHandle() const { return m_LayoutHandle; }

        // Get the raw descriptor set layout (backend-specific opaque pointer)
        void* GetDescriptorSetLayout() const;

        // Get maximum number of textures supported
        uint32_t GetMaxTextures() const { return m_MaxTextures; }

        // Get number of textures currently registered
        uint32_t GetTextureCount() const { return m_TextureCount; }

    private:
        BindlessTextureManager() = default;
        ~BindlessTextureManager() = default;

        // Prevent copying
        BindlessTextureManager(const BindlessTextureManager&) = delete;
        BindlessTextureManager& operator=(const BindlessTextureManager&) = delete;

        uint32_t m_MaxTextures = 0;
        uint32_t m_TextureCount = 0;
        uint32_t m_NextIndex = 0;

        // Sampler filter settings
        Filter m_MinFilter = Filter::Nearest;
        Filter m_MagFilter = Filter::Nearest;

        // RHI handles for descriptor resources
        RHISamplerHandle m_SharedSampler;
        RHIDescriptorSetLayoutHandle m_LayoutHandle;
        RHIDescriptorSetHandle m_DescriptorSetHandle;

        // Free list for recycled indices
        std::queue<BindlessTextureIndex> m_FreeIndices;

        // Map from texture RHI handle ID to bindless index
        std::unordered_map<uint64_t, BindlessTextureIndex> m_HandleToIndex;

        bool m_Initialized = false;
    };

}
