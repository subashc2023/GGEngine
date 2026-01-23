#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"

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
        void Init(uint32_t maxTextures = 16384);
        void Shutdown();

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

        // Vulkan resources (stored as void* to avoid including vulkan.h in header)
        void* m_DescriptorPool = nullptr;
        void* m_DescriptorSetLayout = nullptr;
        void* m_DescriptorSet = nullptr;
        void* m_SharedSampler = nullptr;  // Single sampler shared by all textures

        // RHI handle for the layout
        RHIDescriptorSetLayoutHandle m_LayoutHandle;

        // Free list for recycled indices
        std::queue<BindlessTextureIndex> m_FreeIndices;

        // Map from texture RHI handle ID to bindless index
        std::unordered_map<uint64_t, BindlessTextureIndex> m_HandleToIndex;

        bool m_Initialized = false;
    };

}
