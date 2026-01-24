#pragma once

#include "Asset.h"
#include "AssetManager.h"
#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"
#include "GGEngine/Renderer/BindlessTextureManager.h"

#include <string>
#include <vector>

namespace GGEngine {

    // Template specialization for Texture type
    class Texture;
    template<>
    constexpr AssetType GetAssetType<Texture>() { return AssetType::Texture; }

    // CPU-side loaded texture data (for async loading)
    struct GG_API TextureCPUData
    {
        std::vector<uint8_t> pixels;
        uint32_t width = 0;
        uint32_t height = 0;
        uint32_t channels = 4;
        std::string sourcePath;

        bool IsValid() const { return !pixels.empty() && width > 0 && height > 0; }
    };

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
        static Scope<Texture> Create(uint32_t width, uint32_t height, const void* data,
                                     Filter minFilter = Filter::Nearest, Filter magFilter = Filter::Nearest);

        // Fallback texture (magenta/black checkerboard for missing textures)
        static void InitFallback();
        static void ShutdownFallback();
        static Texture* GetFallbackPtr();

        AssetType GetType() const override { return AssetType::Texture; }

        // Load from image file (PNG, JPG, BMP, TGA supported)
        // This is synchronous - calls LoadCPU then UploadGPU
        bool Load(const std::string& path);

        // ================================================================
        // Async Loading Support
        // ================================================================

        // Load image file to CPU memory (thread-safe, can run on worker thread)
        // Returns TextureCPUData with pixel data, or empty data on failure
        static TextureCPUData LoadCPU(const std::string& path);

        // Upload CPU data to GPU and create resources (must run on main thread)
        // Takes ownership of cpuData pixels
        bool UploadGPU(TextureCPUData&& cpuData);

        // Get source path for hot reload
        const std::string& GetSourcePath() const { return m_SourcePath; }

#ifndef GG_DIST
        // Reload texture from disk, preserving bindless index
        // Available in Debug and Release builds, excluded from Dist
        bool Reload();
#endif

        // ================================================================

        // Set filter mode (must be called before Load or CreateResources)
        void SetFilter(Filter minFilter, Filter magFilter) { m_MinFilter = minFilter; m_MagFilter = magFilter; }
        Filter GetMinFilter() const { return m_MinFilter; }
        Filter GetMagFilter() const { return m_MagFilter; }

        void Unload() override;

        // Accessors
        uint32_t GetWidth() const { return m_Width; }
        uint32_t GetHeight() const { return m_Height; }
        uint32_t GetChannels() const { return m_Channels; }
        TextureFormat GetFormat() const { return m_Format; }

        // RHI handles for backend-agnostic usage
        RHITextureHandle GetHandle() const { return m_Handle; }
        RHISamplerHandle GetSamplerHandle() const { return m_SamplerHandle; }

        // Bindless texture index for shader access
        BindlessTextureIndex GetBindlessIndex() const { return m_BindlessIndex; }

    private:
        void CreateResources(const uint8_t* pixels);

        uint32_t m_Width = 0;
        uint32_t m_Height = 0;
        uint32_t m_Channels = 4;
        TextureFormat m_Format = TextureFormat::R8G8B8A8_UNORM;

        // RHI handles (maps to backend resources via registry)
        RHITextureHandle m_Handle;
        RHISamplerHandle m_SamplerHandle;

        // Sampler settings
        Filter m_MinFilter = Filter::Nearest;
        Filter m_MagFilter = Filter::Nearest;

        // Bindless texture index for shader access
        BindlessTextureIndex m_BindlessIndex = InvalidBindlessIndex;

        // Source path for hot reload
        std::string m_SourcePath;

        // Fallback texture (owned directly, not through AssetManager)
        static Scope<Texture> s_FallbackTexture;
    };

}
