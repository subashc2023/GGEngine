#include "ggpch.h"
#include "Texture.h"
#include "AssetManager.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Renderer/Buffer.h"

#include <stb_image.h>

namespace GGEngine {

    AssetHandle<Texture> Texture::Create(const std::string& path)
    {
        return AssetManager::Get().Load<Texture>(path);
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
            VkImageMemoryBarrier barrier{};
            barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            barrier.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
            barrier.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.image = m_Image;
            barrier.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            barrier.subresourceRange.baseMipLevel = 0;
            barrier.subresourceRange.levelCount = 1;
            barrier.subresourceRange.baseArrayLayer = 0;
            barrier.subresourceRange.layerCount = 1;
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;

            vkCmdPipelineBarrier(cmd,
                VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
                VK_PIPELINE_STAGE_TRANSFER_BIT,
                0,
                0, nullptr,
                0, nullptr,
                1, &barrier);

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
            barrier.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
            barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;

            vkCmdPipelineBarrier(cmd,
                VK_PIPELINE_STAGE_TRANSFER_BIT,
                VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                0,
                0, nullptr,
                0, nullptr,
                1, &barrier);
        });

        // Staging buffer is automatically destroyed when it goes out of scope

        // 4. Create image view
        VkImageViewCreateInfo viewInfo{};
        viewInfo.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
        viewInfo.image = m_Image;
        viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
        viewInfo.format = VK_FORMAT_R8G8B8A8_UNORM;
        viewInfo.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
        viewInfo.subresourceRange.baseMipLevel = 0;
        viewInfo.subresourceRange.levelCount = 1;
        viewInfo.subresourceRange.baseArrayLayer = 0;
        viewInfo.subresourceRange.layerCount = 1;

        if (vkCreateImageView(device, &viewInfo, nullptr, &m_ImageView) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create texture image view!");
            return;
        }

        // 5. Create sampler
        VkSamplerCreateInfo samplerInfo{};
        samplerInfo.sType = VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO;
        samplerInfo.magFilter = VK_FILTER_NEAREST;
        samplerInfo.minFilter = VK_FILTER_NEAREST;
        samplerInfo.addressModeU = VK_SAMPLER_ADDRESS_MODE_REPEAT;
        samplerInfo.addressModeV = VK_SAMPLER_ADDRESS_MODE_REPEAT;
        samplerInfo.addressModeW = VK_SAMPLER_ADDRESS_MODE_REPEAT;
        samplerInfo.anisotropyEnable = VK_FALSE;
        samplerInfo.maxAnisotropy = 1.0f;
        samplerInfo.borderColor = VK_BORDER_COLOR_INT_OPAQUE_BLACK;
        samplerInfo.unnormalizedCoordinates = VK_FALSE;
        samplerInfo.compareEnable = VK_FALSE;
        samplerInfo.compareOp = VK_COMPARE_OP_ALWAYS;
        samplerInfo.mipmapMode = VK_SAMPLER_MIPMAP_MODE_LINEAR;
        samplerInfo.mipLodBias = 0.0f;
        samplerInfo.minLod = 0.0f;
        samplerInfo.maxLod = 0.0f;

        if (vkCreateSampler(device, &samplerInfo, nullptr, &m_Sampler) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create texture sampler!");
            return;
        }
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
