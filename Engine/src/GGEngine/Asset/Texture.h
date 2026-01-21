#pragma once

#include "Asset.h"
#include "AssetManager.h"
#include "GGEngine/Core/Core.h"

#include <vulkan/vulkan.h>
#include <vk_mem_alloc.h>
#include <string>

namespace GGEngine {

    // Template specialization for Texture type
    class Texture;
    template<>
    constexpr AssetType GetAssetType<Texture>() { return AssetType::Texture; }

    // Texture specification for creation
    struct GG_API TextureSpecification
    {
        uint32_t Width = 1;
        uint32_t Height = 1;
        VkFormat Format = VK_FORMAT_R8G8B8A8_UNORM;
        VkFilter MinFilter = VK_FILTER_LINEAR;
        VkFilter MagFilter = VK_FILTER_LINEAR;
        VkSamplerAddressMode AddressMode = VK_SAMPLER_ADDRESS_MODE_REPEAT;
    };

    // A 2D texture asset loaded from an image file
    class GG_API Texture : public Asset
    {
    public:
        Texture() = default;
        ~Texture();

        // Factory method - create texture via asset system
        static AssetHandle<Texture> Create(const std::string& path);

        AssetType GetType() const override { return AssetType::Texture; }

        // Load from image file (PNG, JPG, BMP, TGA supported)
        bool Load(const std::string& path);

        void Unload() override;

        // Accessors
        uint32_t GetWidth() const { return m_Width; }
        uint32_t GetHeight() const { return m_Height; }
        uint32_t GetChannels() const { return m_Channels; }

        VkImage GetImage() const { return m_Image; }
        VkImageView GetImageView() const { return m_ImageView; }
        VkSampler GetSampler() const { return m_Sampler; }

        // Get descriptor info for binding to descriptor sets
        VkDescriptorImageInfo GetDescriptorInfo() const;

    private:
        void CreateVulkanResources(const uint8_t* pixels);

        uint32_t m_Width = 0;
        uint32_t m_Height = 0;
        uint32_t m_Channels = 4;

        VkImage m_Image = VK_NULL_HANDLE;
        VmaAllocation m_ImageAllocation = VK_NULL_HANDLE;
        VkImageView m_ImageView = VK_NULL_HANDLE;
        VkSampler m_Sampler = VK_NULL_HANDLE;
    };

}
