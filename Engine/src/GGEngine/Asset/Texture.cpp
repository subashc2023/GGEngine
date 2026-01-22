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
        s_FallbackTexture->m_Loaded = true;

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

    Scope<Texture> Texture::Create(uint32_t width, uint32_t height, const void* data)
    {
        GG_CORE_ASSERT(data != nullptr, "Texture::Create - data cannot be null");
        GG_CORE_ASSERT(width > 0 && height > 0, "Texture::Create - invalid dimensions");

        auto texture = CreateScope<Texture>();
        texture->m_Width = width;
        texture->m_Height = height;
        texture->m_Channels = 4;  // Always RGBA
        texture->m_Format = TextureFormat::R8G8B8A8_UNORM;
        texture->m_Path = "__generated__";
        texture->m_Loaded = true;

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
        // Resolve path through asset manager
        auto resolvedPath = AssetManager::Get().ResolvePath(path);
        std::string pathStr = resolvedPath.string();

        // Load image using stb_image
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
            return false;
        }

        m_Width = static_cast<uint32_t>(width);
        m_Height = static_cast<uint32_t>(height);
        m_Channels = 4;  // We always request RGBA
        m_Format = TextureFormat::R8G8B8A8_UNORM;
        m_Path = path;

        CreateResources(pixels);

        stbi_image_free(pixels);

        m_Loaded = true;
        GG_CORE_INFO("Texture loaded: {} ({}x{}, {} channels)", path, m_Width, m_Height, channels);
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

        // 3. Create sampler (nearest filtering for pixel-art style)
        RHISamplerSpecification samplerSpec;
        samplerSpec.minFilter = Filter::Nearest;
        samplerSpec.magFilter = Filter::Nearest;
        samplerSpec.mipmapMode = MipmapMode::Nearest;
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

        m_Loaded = false;
    }

}
