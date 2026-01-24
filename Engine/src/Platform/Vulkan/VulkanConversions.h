#pragma once

#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"

#include <vulkan/vulkan.h>

namespace GGEngine {

    // ============================================================================
    // Vulkan Type Conversions
    // ============================================================================
    // Convert RHI enums to Vulkan equivalents

    GG_API VkPrimitiveTopology ToVulkan(PrimitiveTopology topology);
    GG_API VkPolygonMode ToVulkan(PolygonMode mode);
    GG_API VkCullModeFlags ToVulkan(CullMode mode);
    GG_API VkFrontFace ToVulkan(FrontFace face);
    GG_API VkCompareOp ToVulkan(CompareOp op);
    GG_API VkSampleCountFlagBits ToVulkan(SampleCount count);
    GG_API VkShaderStageFlags ToVulkan(ShaderStage stages);
    GG_API VkDescriptorType ToVulkan(DescriptorType type);
    GG_API VkFormat ToVulkan(TextureFormat format);
    GG_API VkFilter ToVulkan(Filter filter);
    GG_API VkSamplerMipmapMode ToVulkan(MipmapMode mode);
    GG_API VkSamplerAddressMode ToVulkan(AddressMode mode);
    GG_API VkBorderColor ToVulkan(BorderColor color);
    GG_API VkIndexType ToVulkan(IndexType type);
    GG_API VkBlendFactor ToVulkan(BlendFactor factor);
    GG_API VkBlendOp ToVulkan(BlendOp op);
    GG_API VkColorComponentFlags ToVulkan(ColorComponent components);
    GG_API VkAttachmentLoadOp ToVulkan(LoadOp op);
    GG_API VkAttachmentStoreOp ToVulkan(StoreOp op);
    GG_API VkImageLayout ToVulkan(ImageLayout layout);

    // Vertex input conversions
    GG_API VkVertexInputRate ToVulkan(VertexInputRate rate);
    GG_API VkVertexInputBindingDescription ToVulkan(const RHIVertexBindingDescription& desc);
    GG_API VkVertexInputAttributeDescription ToVulkan(const RHIVertexAttributeDescription& desc);

    // Buffer usage conversion (special case - maps to multiple flags)
    GG_API VkBufferUsageFlags ToVulkanBufferUsage(BufferUsage usage, bool cpuVisible);

    // Reverse conversions (Vulkan to RHI)
    GG_API TextureFormat FromVulkan(VkFormat format);
    GG_API ShaderStage FromVulkanShaderStage(VkShaderStageFlagBits stage);

}
