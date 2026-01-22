#pragma once

#include "GGEngine/Core/Log.h"
#include <vulkan/vulkan.h>

namespace GGEngine {

    // Log Vulkan error and continue execution
    #define VK_CHECK(result, msg) \
        do { \
            if ((result) != VK_SUCCESS) { \
                GG_CORE_ERROR("Vulkan error: {} (code: {})", msg, static_cast<int>(result)); \
            } \
        } while(0)

    // Log Vulkan error and return from function
    #define VK_CHECK_RETURN(result, msg) \
        do { \
            if ((result) != VK_SUCCESS) { \
                GG_CORE_ERROR("Vulkan error: {} (code: {})", msg, static_cast<int>(result)); \
                return; \
            } \
        } while(0)

    // Log Vulkan error and return a value from function
    #define VK_CHECK_RETURN_VAL(result, msg, retval) \
        do { \
            if ((result) != VK_SUCCESS) { \
                GG_CORE_ERROR("Vulkan error: {} (code: {})", msg, static_cast<int>(result)); \
                return retval; \
            } \
        } while(0)

    namespace VulkanUtils {

        // Create a 2D image view with common defaults
        inline VkImageView CreateImageView2D(VkDevice device, VkImage image, VkFormat format,
                                             VkImageAspectFlags aspectMask = VK_IMAGE_ASPECT_COLOR_BIT)
        {
            VkImageViewCreateInfo viewInfo{};
            viewInfo.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
            viewInfo.image = image;
            viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
            viewInfo.format = format;
            viewInfo.subresourceRange.aspectMask = aspectMask;
            viewInfo.subresourceRange.baseMipLevel = 0;
            viewInfo.subresourceRange.levelCount = 1;
            viewInfo.subresourceRange.baseArrayLayer = 0;
            viewInfo.subresourceRange.layerCount = 1;

            VkImageView imageView;
            if (vkCreateImageView(device, &viewInfo, nullptr, &imageView) != VK_SUCCESS)
            {
                GG_CORE_ERROR("Failed to create image view!");
                return VK_NULL_HANDLE;
            }
            return imageView;
        }

        // Create a sampler with common defaults
        inline VkSampler CreateSampler(VkDevice device,
                                       VkFilter filter = VK_FILTER_LINEAR,
                                       VkSamplerAddressMode addressMode = VK_SAMPLER_ADDRESS_MODE_REPEAT)
        {
            VkSamplerCreateInfo samplerInfo{};
            samplerInfo.sType = VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO;
            samplerInfo.magFilter = filter;
            samplerInfo.minFilter = filter;
            samplerInfo.addressModeU = addressMode;
            samplerInfo.addressModeV = addressMode;
            samplerInfo.addressModeW = addressMode;
            samplerInfo.anisotropyEnable = VK_FALSE;
            samplerInfo.maxAnisotropy = 1.0f;
            samplerInfo.borderColor = VK_BORDER_COLOR_INT_OPAQUE_BLACK;
            samplerInfo.unnormalizedCoordinates = VK_FALSE;
            samplerInfo.compareEnable = VK_FALSE;
            samplerInfo.compareOp = VK_COMPARE_OP_ALWAYS;
            samplerInfo.mipmapMode = (filter == VK_FILTER_LINEAR)
                ? VK_SAMPLER_MIPMAP_MODE_LINEAR
                : VK_SAMPLER_MIPMAP_MODE_NEAREST;
            samplerInfo.mipLodBias = 0.0f;
            samplerInfo.minLod = 0.0f;
            samplerInfo.maxLod = 0.0f;

            VkSampler sampler;
            if (vkCreateSampler(device, &samplerInfo, nullptr, &sampler) != VK_SUCCESS)
            {
                GG_CORE_ERROR("Failed to create sampler!");
                return VK_NULL_HANDLE;
            }
            return sampler;
        }

        // Record an image layout transition barrier into a command buffer
        inline void TransitionImageLayout(VkCommandBuffer cmd, VkImage image,
                                          VkImageLayout oldLayout, VkImageLayout newLayout,
                                          VkImageAspectFlags aspectMask = VK_IMAGE_ASPECT_COLOR_BIT)
        {
            VkImageMemoryBarrier barrier{};
            barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            barrier.oldLayout = oldLayout;
            barrier.newLayout = newLayout;
            barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.image = image;
            barrier.subresourceRange.aspectMask = aspectMask;
            barrier.subresourceRange.baseMipLevel = 0;
            barrier.subresourceRange.levelCount = 1;
            barrier.subresourceRange.baseArrayLayer = 0;
            barrier.subresourceRange.layerCount = 1;

            VkPipelineStageFlags srcStage, dstStage;

            if (oldLayout == VK_IMAGE_LAYOUT_UNDEFINED && newLayout == VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL)
            {
                barrier.srcAccessMask = 0;
                barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
                srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
                dstStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
            }
            else if (oldLayout == VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL && newLayout == VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL)
            {
                barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
                barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
                srcStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
                dstStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            }
            else if (oldLayout == VK_IMAGE_LAYOUT_UNDEFINED && newLayout == VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL)
            {
                barrier.srcAccessMask = 0;
                barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
                srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
                dstStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            }
            else
            {
                GG_CORE_ERROR("Unsupported image layout transition: {} -> {}",
                    static_cast<int>(oldLayout), static_cast<int>(newLayout));
                return;
            }

            vkCmdPipelineBarrier(cmd, srcStage, dstStage, 0,
                0, nullptr, 0, nullptr, 1, &barrier);
        }

    } // namespace VulkanUtils

}
