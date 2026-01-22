#include "ggpch.h"
#include "Texture.h"
#include "AssetManager.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "Platform/Vulkan/VulkanUtils.h"
#include "GGEngine/Renderer/Buffer.h"

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
        s_FallbackTexture->m_Path = "__fallback__";
        s_FallbackTexture->m_Loaded = true;

        s_FallbackTexture->CreateVulkanResources(pixels.data());

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
        texture->m_Path = "__generated__";
        texture->m_Loaded = true;

        texture->CreateVulkanResources(static_cast<const uint8_t*>(data));

        GG_CORE_TRACE("Created {}x{} texture from raw pixel data", width, height);
        return texture;
    }

    Texture::~Texture()
    {
        Unload();
    }

    bool Texture::Load(const std::string& path)
    {
        // Resolve path through asset manager
        auto resolvedPath = AssetManager::Get().ResolvePath(path);
        std::string pathStr = resolvedPath.string();

        // Load image using stb_image
        int width, height, channels;
        stbi_set_flip_vertically_on_load(1);  // Vulkan expects bottom-left origin like OpenGL

        unsigned char* pixels = stbi_load(pathStr.c_str(), &width, &height, &channels, STBI_rgb_alpha);

        if (!pixels)
        {
            GG_CORE_ERROR("Failed to load texture: {} - {}", path, stbi_failure_reason());
            return false;
        }

        m_Width = static_cast<uint32_t>(width);
        m_Height = static_cast<uint32_t>(height);
        m_Channels = 4;  // We always request RGBA
        m_Path = path;

        CreateVulkanResources(pixels);

        stbi_image_free(pixels);

        m_Loaded = true;
        GG_CORE_INFO("Texture loaded: {} ({}x{}, {} channels)", path, m_Width, m_Height, channels);
        return true;
    }

    void Texture::CreateVulkanResources(const uint8_t* pixels)
    {
        auto& ctx = VulkanContext::Get();
        VkDevice device = ctx.GetDevice();
        VmaAllocator allocator = ctx.GetAllocator();

        VkDeviceSize imageSize = m_Width * m_Height * 4;

        // 1. Create staging buffer with pixel data
        BufferSpecification stagingSpec;
        stagingSpec.size = imageSize;
        stagingSpec.usage = BufferUsage::Staging;
        stagingSpec.cpuVisible = true;
        stagingSpec.debugName = "TextureStagingBuffer";

        Buffer stagingBuffer(stagingSpec);
        stagingBuffer.SetData(pixels, imageSize);

        // 2. Create image with VMA
        VkImageCreateInfo imageInfo{};
        imageInfo.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
        imageInfo.imageType = VK_IMAGE_TYPE_2D;
        imageInfo.extent.width = m_Width;
        imageInfo.extent.height = m_Height;
        imageInfo.extent.depth = 1;
        imageInfo.mipLevels = 1;
        imageInfo.arrayLayers = 1;
        imageInfo.format = VK_FORMAT_R8G8B8A8_UNORM;
        imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
        imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        imageInfo.usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT;
        imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
        imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;

        VmaAllocationCreateInfo allocCreateInfo{};
        allocCreateInfo.usage = VMA_MEMORY_USAGE_AUTO;
        allocCreateInfo.flags = VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT;

        if (vmaCreateImage(allocator, &imageInfo, &allocCreateInfo, &m_Image, &m_ImageAllocation, nullptr) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create texture image with VMA!");
            return;
        }

        // 3. Transition, copy, transition via ImmediateSubmit
        VkBuffer stagingVkBuffer = stagingBuffer.GetVkBuffer();

        ctx.ImmediateSubmit([&](VkCommandBuffer cmd) {
            // UNDEFINED -> TRANSFER_DST_OPTIMAL
            VulkanUtils::TransitionImageLayout(cmd, m_Image,
                VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL);

            // Copy buffer to image
            VkBufferImageCopy region{};
            region.bufferOffset = 0;
            region.bufferRowLength = 0;
            region.bufferImageHeight = 0;
            region.imageSubresource.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            region.imageSubresource.mipLevel = 0;
            region.imageSubresource.baseArrayLayer = 0;
            region.imageSubresource.layerCount = 1;
            region.imageOffset = { 0, 0, 0 };
            region.imageExtent = { m_Width, m_Height, 1 };

            vkCmdCopyBufferToImage(cmd, stagingVkBuffer, m_Image,
                VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);

            // TRANSFER_DST_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL
            VulkanUtils::TransitionImageLayout(cmd, m_Image,
                VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);
        });

        // Staging buffer is automatically destroyed when it goes out of scope

        // 4. Create image view
        m_ImageView = VulkanUtils::CreateImageView2D(device, m_Image, VK_FORMAT_R8G8B8A8_UNORM);
        if (m_ImageView == VK_NULL_HANDLE)
            return;

        // 5. Create sampler (nearest filtering for pixel-art style)
        m_Sampler = VulkanUtils::CreateSampler(device, VK_FILTER_NEAREST, VK_SAMPLER_ADDRESS_MODE_REPEAT);
        if (m_Sampler == VK_NULL_HANDLE)
            return;
    }

    void Texture::Unload()
    {
        if (m_Image == VK_NULL_HANDLE)
            return;

        // Check if VulkanContext is still valid (may be destroyed during static destruction)
        VkDevice device = VulkanContext::Get().GetDevice();
        if (device == VK_NULL_HANDLE)
        {
            m_Image = VK_NULL_HANDLE;
            m_ImageAllocation = VK_NULL_HANDLE;
            m_ImageView = VK_NULL_HANDLE;
            m_Sampler = VK_NULL_HANDLE;
            m_Loaded = false;
            return;
        }

        if (m_Sampler != VK_NULL_HANDLE)
        {
            vkDestroySampler(device, m_Sampler, nullptr);
            m_Sampler = VK_NULL_HANDLE;
        }

        if (m_ImageView != VK_NULL_HANDLE)
        {
            vkDestroyImageView(device, m_ImageView, nullptr);
            m_ImageView = VK_NULL_HANDLE;
        }

        if (m_Image != VK_NULL_HANDLE)
        {
            VmaAllocator allocator = VulkanContext::Get().GetAllocator();
            vmaDestroyImage(allocator, m_Image, m_ImageAllocation);
            m_Image = VK_NULL_HANDLE;
            m_ImageAllocation = VK_NULL_HANDLE;
        }

        m_Loaded = false;
    }

    VkDescriptorImageInfo Texture::GetDescriptorInfo() const
    {
        VkDescriptorImageInfo info{};
        info.sampler = m_Sampler;
        info.imageView = m_ImageView;
        info.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
        return info;
    }

}
