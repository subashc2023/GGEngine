#pragma once

#include "Asset.h"
#include "AssetManager.h"
#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"

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
        TextureFormat Format = TextureFormat::R8G8B8A8_UNORM;
        Filter MinFilter = Filter::Linear;
        Filter MagFilter = Filter::Linear;
        AddressMode AddressModeU = AddressMode::Repeat;
        AddressMode AddressModeV = AddressMode::Repeat;
    };

    // A 2D texture asset loaded from an image file
    class GG_API Texture : public Asset
    {
    public:
        Texture() = default;
        ~Texture();

        // Factory method - create texture via asset system
        static AssetHandle<Texture> Create(const std::string& path);

        // Factory method - create texture from raw pixel data (not managed by AssetManager)
        // Useful for programmatic textures like 1x1 white pixel
        static Scope<Texture> Create(uint32_t width, uint32_t height, const void* data);

        // Fallback texture (magenta/black checkerboard for missing textures)
        static void InitFallback();
        static void ShutdownFallback();
        static Texture* GetFallbackPtr();

        AssetType GetType() const override { return AssetType::Texture; }

        // Load from image file (PNG, JPG, BMP, TGA supported)
        bool Load(const std::string& path);

        void Unload() override;

        // Accessors
        uint32_t GetWidth() const { return m_Width; }
        uint32_t GetHeight() const { return m_Height; }
        uint32_t GetChannels() const { return m_Channels; }
        TextureFormat GetFormat() const { return m_Format; }

        // RHI handle for backend-agnostic usage
        RHITextureHandle GetHandle() const { return m_Handle; }

    private:
        void CreateVulkanResources(const uint8_t* pixels);

        uint32_t m_Width = 0;
        uint32_t m_Height = 0;
        uint32_t m_Channels = 4;
        TextureFormat m_Format = TextureFormat::R8G8B8A8_UNORM;

        // RHI handle (maps to backend resources via registry)
        RHITextureHandle m_Handle;

        // Fallback texture (owned directly, not through AssetManager)
        static Scope<Texture> s_FallbackTexture;
    };

}
