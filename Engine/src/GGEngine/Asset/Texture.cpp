#include "ggpch.h"
#include "Texture.h"
#include "GGEngine/Core/Profiler.h"
#include "AssetManager.h"
#include "GGEngine/RHI/RHIDevice.h"

#include <stb_image.h>
#include <vector>

namespace GGEngine {

    // Static member initialization
    Scope<Texture> Texture::s_FallbackTexture;

    void Texture::InitFallback()
    {
        if (s_FallbackTexture)
            return; // Already initialized

        // Create 8x8 magenta/black checkerboard
        const uint32_t size = 8;
        const uint32_t checkerSize = 2; // 2x2 pixel squares
        std::vector<uint8_t> pixels(size * size * 4);

        for (uint32_t y = 0; y < size; y++)
        {
            for (uint32_t x = 0; x < size; x++)
            {
                uint32_t index = (y * size + x) * 4;

                // Determine if this pixel is magenta or black based on checker pattern
                bool isMagenta = ((x / checkerSize) + (y / checkerSize)) % 2 == 0;

                if (isMagenta)
                {
                    pixels[index + 0] = 255; // R
                    pixels[index + 1] = 0;   // G
                    pixels[index + 2] = 255; // B
                    pixels[index + 3] = 255; // A
                }
                else
                {
                    pixels[index + 0] = 0;   // R
                    pixels[index + 1] = 0;   // G
                    pixels[index + 2] = 0;   // B
                    pixels[index + 3] = 255; // A
                }
            }
        }

        // Create texture directly without going through AssetManager
        s_FallbackTexture = CreateScope<Texture>();
        s_FallbackTexture->m_Width = size;
        s_FallbackTexture->m_Height = size;
        s_FallbackTexture->m_Channels = 4;
        s_FallbackTexture->m_Format = TextureFormat::R8G8B8A8_UNORM;
        s_FallbackTexture->m_Path = "__fallback__";
        s_FallbackTexture->SetState(AssetState::Ready);

        s_FallbackTexture->CreateResources(pixels.data());

        GG_CORE_INFO("Fallback texture initialized ({}x{} magenta/black checkerboard)", size, size);
    }

    void Texture::ShutdownFallback()
    {
        if (s_FallbackTexture)
        {
            s_FallbackTexture->Unload();
            s_FallbackTexture.reset();
        }
        GG_CORE_TRACE("Fallback texture shutdown");
    }

    Texture* Texture::GetFallbackPtr()
    {
        if (!s_FallbackTexture)
        {
            InitFallback();
        }
        return s_FallbackTexture.get();
    }

    AssetHandle<Texture> Texture::Create(const std::string& path)
    {
        return AssetManager::Get().Load<Texture>(path);
    }

    Scope<Texture> Texture::Create(uint32_t width, uint32_t height, const void* data,
                                    Filter minFilter, Filter magFilter)
    {
        GG_CORE_ASSERT(data != nullptr, "Texture::Create - data cannot be null");
        GG_CORE_ASSERT(width > 0 && height > 0, "Texture::Create - invalid dimensions");

        auto texture = CreateScope<Texture>();
        texture->m_Width = width;
        texture->m_Height = height;
        texture->m_Channels = 4;  // Always RGBA
        texture->m_Format = TextureFormat::R8G8B8A8_UNORM;
        texture->m_Path = "__generated__";
        texture->SetState(AssetState::Ready);
        texture->m_MinFilter = minFilter;
        texture->m_MagFilter = magFilter;

        texture->CreateResources(static_cast<const uint8_t*>(data));

        GG_CORE_TRACE("Created {}x{} texture from raw pixel data", width, height);
        return texture;
    }

    Texture::~Texture()
    {
        Unload();
    }

    bool Texture::Load(const std::string& path)
    {
        GG_PROFILE_SCOPE("Texture::Load");

        // Use the two-phase loading: CPU then GPU
        TextureCPUData cpuData = LoadCPU(path);
        if (!cpuData.IsValid())
        {
            SetError("Failed to load texture from file");
            return false;
        }

        return UploadGPU(std::move(cpuData));
    }

    TextureCPUData Texture::LoadCPU(const std::string& path)
    {
        GG_PROFILE_SCOPE("Texture::LoadCPU");

        TextureCPUData result;
        result.sourcePath = path;

        // Resolve path through asset manager
        auto resolvedPath = AssetManager::Get().ResolvePath(path);
        std::string pathStr = resolvedPath.string();

        // Load image using stb_image (thread-safe as long as we don't share file handles)
        int width, height, channels;
        stbi_set_flip_vertically_on_load(1);  // Vulkan expects bottom-left origin like OpenGL

        unsigned char* pixels;
        {
            GG_PROFILE_SCOPE("stbi_load");
            pixels = stbi_load(pathStr.c_str(), &width, &height, &channels, STBI_rgb_alpha);
        }

        if (!pixels)
        {
            GG_CORE_ERROR("Failed to load texture: {} - {}", path, stbi_failure_reason());
            return result;  // Return empty/invalid data
        }

        // Copy pixel data to our vector (stbi uses malloc, we need to free it)
        uint64_t imageSize = static_cast<uint64_t>(width) * height * 4;
        result.pixels.resize(imageSize);
        std::memcpy(result.pixels.data(), pixels, imageSize);
        result.width = static_cast<uint32_t>(width);
        result.height = static_cast<uint32_t>(height);
        result.channels = 4;  // We always request RGBA

        stbi_image_free(pixels);

        GG_CORE_TRACE("Texture::LoadCPU completed: {} ({}x{})", path, result.width, result.height);
        return result;
    }

    bool Texture::UploadGPU(TextureCPUData&& cpuData)
    {
        GG_PROFILE_SCOPE("Texture::UploadGPU");

        if (!cpuData.IsValid())
        {
            GG_CORE_ERROR("Texture::UploadGPU - invalid CPU data");
            SetError("Invalid CPU data for GPU upload");
            return false;
        }

        m_Width = cpuData.width;
        m_Height = cpuData.height;
        m_Channels = cpuData.channels;
        m_Format = TextureFormat::R8G8B8A8_UNORM;
        m_Path = cpuData.sourcePath;
        m_SourcePath = cpuData.sourcePath;

        CreateResources(cpuData.pixels.data());

        // Clear CPU data to free memory (pixels moved, now release)
        cpuData.pixels.clear();
        cpuData.pixels.shrink_to_fit();

        SetState(AssetState::Ready);
        GG_CORE_INFO("Texture uploaded to GPU: {} ({}x{})", m_SourcePath, m_Width, m_Height);
        return true;
    }

    void Texture::CreateResources(const uint8_t* pixels)
    {
        auto& device = RHIDevice::Get();
        uint64_t imageSize = m_Width * m_Height * 4;

        // 1. Create texture through RHI device
        RHITextureSpecification textureSpec;
        textureSpec.width = m_Width;
        textureSpec.height = m_Height;
        textureSpec.depth = 1;
        textureSpec.mipLevels = 1;
        textureSpec.arrayLayers = 1;
        textureSpec.format = m_Format;
        textureSpec.samples = SampleCount::Count1;
        textureSpec.usage = TextureUsage::Sampled | TextureUsage::TransferDst;
        textureSpec.debugName = m_Path.string();

        m_Handle = device.CreateTexture(textureSpec);
        if (!m_Handle.IsValid())
        {
            GG_CORE_ERROR("Failed to create texture through RHI!");
            return;
        }

        // 2. Upload pixel data (handles staging, layout transitions internally)
        device.UploadTextureData(m_Handle, pixels, imageSize);

        // 3. Create sampler with configured filtering
        RHISamplerSpecification samplerSpec;
        samplerSpec.minFilter = m_MinFilter;
        samplerSpec.magFilter = m_MagFilter;
        samplerSpec.mipmapMode = (m_MinFilter == Filter::Nearest) ? MipmapMode::Nearest : MipmapMode::Linear;
        samplerSpec.addressModeU = AddressMode::Repeat;
        samplerSpec.addressModeV = AddressMode::Repeat;
        samplerSpec.addressModeW = AddressMode::Repeat;

        m_SamplerHandle = device.CreateSampler(samplerSpec);
        if (!m_SamplerHandle.IsValid())
        {
            device.DestroyTexture(m_Handle);
            m_Handle = NullTexture;
            GG_CORE_ERROR("Failed to create sampler through RHI!");
            return;
        }

        // 4. Register with BindlessTextureManager for bindless rendering
        // Only register if the manager is initialized (it may not be during fallback texture creation)
        auto& bindlessManager = BindlessTextureManager::Get();
        if (bindlessManager.GetMaxTextures() > 0)
        {
            m_BindlessIndex = bindlessManager.RegisterTexture(*this);
        }
    }

#ifndef GG_DIST
    bool Texture::Reload()
    {
        GG_PROFILE_SCOPE("Texture::Reload");

        if (m_SourcePath.empty())
        {
            GG_CORE_WARN("Cannot reload texture without source path");
            return false;
        }

        // Save the bindless index before unloading
        BindlessTextureIndex savedIndex = m_BindlessIndex;
        Filter savedMinFilter = m_MinFilter;
        Filter savedMagFilter = m_MagFilter;

        GG_CORE_INFO("Hot reloading texture: {} (bindless index: {})", m_SourcePath, savedIndex);

        SetState(AssetState::Reloading);

        // Destroy GPU resources WITHOUT unregistering from bindless manager
        // (we want to reuse the same slot)
        if (m_SamplerHandle.IsValid())
        {
            RHIDevice::Get().DestroySampler(m_SamplerHandle);
            m_SamplerHandle = NullSampler;
        }

        if (m_Handle.IsValid())
        {
            RHIDevice::Get().DestroyTexture(m_Handle);
            m_Handle = NullTexture;
        }

        // Reload from disk (synchronous for simplicity - hot reload is dev-only)
        TextureCPUData cpuData = LoadCPU(m_SourcePath);
        if (!cpuData.IsValid())
        {
            SetError("Failed to reload texture from disk");
            GG_CORE_ERROR("Hot reload failed for texture: {}", m_SourcePath);
            return false;
        }

        // Store dimensions and path
        m_Width = cpuData.width;
        m_Height = cpuData.height;
        m_Channels = cpuData.channels;
        m_MinFilter = savedMinFilter;
        m_MagFilter = savedMagFilter;

        // Create new GPU resources
        auto& device = RHIDevice::Get();
        uint64_t imageSize = m_Width * m_Height * 4;

        RHITextureSpecification textureSpec;
        textureSpec.width = m_Width;
        textureSpec.height = m_Height;
        textureSpec.depth = 1;
        textureSpec.mipLevels = 1;
        textureSpec.arrayLayers = 1;
        textureSpec.format = m_Format;
        textureSpec.samples = SampleCount::Count1;
        textureSpec.usage = TextureUsage::Sampled | TextureUsage::TransferDst;
        textureSpec.debugName = m_Path.string();

        m_Handle = device.CreateTexture(textureSpec);
        if (!m_Handle.IsValid())
        {
            SetError("Failed to create texture during reload");
            return false;
        }

        device.UploadTextureData(m_Handle, cpuData.pixels.data(), imageSize);

        RHISamplerSpecification samplerSpec;
        samplerSpec.minFilter = m_MinFilter;
        samplerSpec.magFilter = m_MagFilter;
        samplerSpec.mipmapMode = (m_MinFilter == Filter::Nearest) ? MipmapMode::Nearest : MipmapMode::Linear;
        samplerSpec.addressModeU = AddressMode::Repeat;
        samplerSpec.addressModeV = AddressMode::Repeat;
        samplerSpec.addressModeW = AddressMode::Repeat;

        m_SamplerHandle = device.CreateSampler(samplerSpec);
        if (!m_SamplerHandle.IsValid())
        {
            device.DestroyTexture(m_Handle);
            m_Handle = NullTexture;
            SetError("Failed to create sampler during reload");
            return false;
        }

        // Re-register at the same bindless index to preserve shader references
        if (savedIndex != InvalidBindlessIndex)
        {
            m_BindlessIndex = BindlessTextureManager::Get().RegisterTextureAtIndex(*this, savedIndex);
            if (m_BindlessIndex != savedIndex)
            {
                GG_CORE_WARN("Bindless index changed during hot reload: {} -> {}", savedIndex, m_BindlessIndex);
            }
        }
        else
        {
            // Texture wasn't registered before, register now
            auto& bindlessManager = BindlessTextureManager::Get();
            if (bindlessManager.GetMaxTextures() > 0)
            {
                m_BindlessIndex = bindlessManager.RegisterTexture(*this);
            }
        }

        SetState(AssetState::Ready);
        GG_CORE_INFO("Hot reload complete: {} ({}x{}, bindless: {})", m_SourcePath, m_Width, m_Height, m_BindlessIndex);
        return true;
    }
#endif

    void Texture::Unload()
    {
        // Unregister from BindlessTextureManager first
        if (m_BindlessIndex != InvalidBindlessIndex)
        {
            BindlessTextureManager::Get().UnregisterTexture(m_BindlessIndex);
            m_BindlessIndex = InvalidBindlessIndex;
        }

        // Destroy sampler through RHI device
        if (m_SamplerHandle.IsValid())
        {
            RHIDevice::Get().DestroySampler(m_SamplerHandle);
            m_SamplerHandle = NullSampler;
        }

        // Destroy texture through RHI device
        if (m_Handle.IsValid())
        {
            RHIDevice::Get().DestroyTexture(m_Handle);
            m_Handle = NullTexture;
        }

        SetState(AssetState::Unloaded);
    }

}
