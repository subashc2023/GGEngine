#include "ggpch.h"
#include "VulkanConversions.h"

namespace GGEngine {

    // ============================================================================
    // Vulkan Type Conversions
    // ============================================================================

    VkPrimitiveTopology ToVulkan(PrimitiveTopology topology)
    {
        switch (topology)
        {
            case PrimitiveTopology::PointList:     return VK_PRIMITIVE_TOPOLOGY_POINT_LIST;
            case PrimitiveTopology::LineList:      return VK_PRIMITIVE_TOPOLOGY_LINE_LIST;
            case PrimitiveTopology::LineStrip:     return VK_PRIMITIVE_TOPOLOGY_LINE_STRIP;
            case PrimitiveTopology::TriangleList:  return VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
            case PrimitiveTopology::TriangleStrip: return VK_PRIMITIVE_TOPOLOGY_TRIANGLE_STRIP;
            case PrimitiveTopology::TriangleFan:   return VK_PRIMITIVE_TOPOLOGY_TRIANGLE_FAN;
            default:                               return VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
        }
    }

    VkPolygonMode ToVulkan(PolygonMode mode)
    {
        switch (mode)
        {
            case PolygonMode::Fill:  return VK_POLYGON_MODE_FILL;
            case PolygonMode::Line:  return VK_POLYGON_MODE_LINE;
            case PolygonMode::Point: return VK_POLYGON_MODE_POINT;
            default:                 return VK_POLYGON_MODE_FILL;
        }
    }

    VkCullModeFlags ToVulkan(CullMode mode)
    {
        switch (mode)
        {
            case CullMode::None:         return VK_CULL_MODE_NONE;
            case CullMode::Front:        return VK_CULL_MODE_FRONT_BIT;
            case CullMode::Back:         return VK_CULL_MODE_BACK_BIT;
            case CullMode::FrontAndBack: return VK_CULL_MODE_FRONT_AND_BACK;
            default:                     return VK_CULL_MODE_NONE;
        }
    }

    VkFrontFace ToVulkan(FrontFace face)
    {
        switch (face)
        {
            case FrontFace::CounterClockwise: return VK_FRONT_FACE_COUNTER_CLOCKWISE;
            case FrontFace::Clockwise:        return VK_FRONT_FACE_CLOCKWISE;
            default:                          return VK_FRONT_FACE_CLOCKWISE;
        }
    }

    VkCompareOp ToVulkan(CompareOp op)
    {
        switch (op)
        {
            case CompareOp::Never:          return VK_COMPARE_OP_NEVER;
            case CompareOp::Less:           return VK_COMPARE_OP_LESS;
            case CompareOp::Equal:          return VK_COMPARE_OP_EQUAL;
            case CompareOp::LessOrEqual:    return VK_COMPARE_OP_LESS_OR_EQUAL;
            case CompareOp::Greater:        return VK_COMPARE_OP_GREATER;
            case CompareOp::NotEqual:       return VK_COMPARE_OP_NOT_EQUAL;
            case CompareOp::GreaterOrEqual: return VK_COMPARE_OP_GREATER_OR_EQUAL;
            case CompareOp::Always:         return VK_COMPARE_OP_ALWAYS;
            default:                        return VK_COMPARE_OP_LESS;
        }
    }

    VkSampleCountFlagBits ToVulkan(SampleCount count)
    {
        switch (count)
        {
            case SampleCount::Count1:  return VK_SAMPLE_COUNT_1_BIT;
            case SampleCount::Count2:  return VK_SAMPLE_COUNT_2_BIT;
            case SampleCount::Count4:  return VK_SAMPLE_COUNT_4_BIT;
            case SampleCount::Count8:  return VK_SAMPLE_COUNT_8_BIT;
            case SampleCount::Count16: return VK_SAMPLE_COUNT_16_BIT;
            case SampleCount::Count32: return VK_SAMPLE_COUNT_32_BIT;
            case SampleCount::Count64: return VK_SAMPLE_COUNT_64_BIT;
            default:                   return VK_SAMPLE_COUNT_1_BIT;
        }
    }

    VkShaderStageFlags ToVulkan(ShaderStage stages)
    {
        VkShaderStageFlags flags = 0;
        if (HasFlag(stages, ShaderStage::Vertex))
            flags |= VK_SHADER_STAGE_VERTEX_BIT;
        if (HasFlag(stages, ShaderStage::TessellationControl))
            flags |= VK_SHADER_STAGE_TESSELLATION_CONTROL_BIT;
        if (HasFlag(stages, ShaderStage::TessellationEvaluation))
            flags |= VK_SHADER_STAGE_TESSELLATION_EVALUATION_BIT;
        if (HasFlag(stages, ShaderStage::Geometry))
            flags |= VK_SHADER_STAGE_GEOMETRY_BIT;
        if (HasFlag(stages, ShaderStage::Fragment))
            flags |= VK_SHADER_STAGE_FRAGMENT_BIT;
        if (HasFlag(stages, ShaderStage::Compute))
            flags |= VK_SHADER_STAGE_COMPUTE_BIT;
        return flags;
    }

    VkDescriptorType ToVulkan(DescriptorType type)
    {
        switch (type)
        {
            case DescriptorType::Sampler:              return VK_DESCRIPTOR_TYPE_SAMPLER;
            case DescriptorType::CombinedImageSampler: return VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
            case DescriptorType::SampledImage:         return VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE;
            case DescriptorType::StorageImage:         return VK_DESCRIPTOR_TYPE_STORAGE_IMAGE;
            case DescriptorType::UniformTexelBuffer:   return VK_DESCRIPTOR_TYPE_UNIFORM_TEXEL_BUFFER;
            case DescriptorType::StorageTexelBuffer:   return VK_DESCRIPTOR_TYPE_STORAGE_TEXEL_BUFFER;
            case DescriptorType::UniformBuffer:        return VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
            case DescriptorType::StorageBuffer:        return VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
            case DescriptorType::UniformBufferDynamic: return VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER_DYNAMIC;
            case DescriptorType::StorageBufferDynamic: return VK_DESCRIPTOR_TYPE_STORAGE_BUFFER_DYNAMIC;
            case DescriptorType::InputAttachment:      return VK_DESCRIPTOR_TYPE_INPUT_ATTACHMENT;
            default:                                   return VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        }
    }

    VkFormat ToVulkan(TextureFormat format)
    {
        switch (format)
        {
            case TextureFormat::Undefined:          return VK_FORMAT_UNDEFINED;
            case TextureFormat::R8_UNORM:           return VK_FORMAT_R8_UNORM;
            case TextureFormat::R8_SNORM:           return VK_FORMAT_R8_SNORM;
            case TextureFormat::R8_UINT:            return VK_FORMAT_R8_UINT;
            case TextureFormat::R8_SINT:            return VK_FORMAT_R8_SINT;
            case TextureFormat::R8G8_UNORM:         return VK_FORMAT_R8G8_UNORM;
            case TextureFormat::R8G8_SNORM:         return VK_FORMAT_R8G8_SNORM;
            case TextureFormat::R8G8_UINT:          return VK_FORMAT_R8G8_UINT;
            case TextureFormat::R8G8_SINT:          return VK_FORMAT_R8G8_SINT;
            case TextureFormat::R8G8B8_UNORM:       return VK_FORMAT_R8G8B8_UNORM;
            case TextureFormat::R8G8B8_SRGB:        return VK_FORMAT_R8G8B8_SRGB;
            case TextureFormat::R8G8B8A8_UNORM:     return VK_FORMAT_R8G8B8A8_UNORM;
            case TextureFormat::R8G8B8A8_SNORM:     return VK_FORMAT_R8G8B8A8_SNORM;
            case TextureFormat::R8G8B8A8_UINT:      return VK_FORMAT_R8G8B8A8_UINT;
            case TextureFormat::R8G8B8A8_SINT:      return VK_FORMAT_R8G8B8A8_SINT;
            case TextureFormat::R8G8B8A8_SRGB:      return VK_FORMAT_R8G8B8A8_SRGB;
            case TextureFormat::B8G8R8A8_UNORM:     return VK_FORMAT_B8G8R8A8_UNORM;
            case TextureFormat::B8G8R8A8_SRGB:      return VK_FORMAT_B8G8R8A8_SRGB;
            case TextureFormat::R16_UNORM:          return VK_FORMAT_R16_UNORM;
            case TextureFormat::R16_SNORM:          return VK_FORMAT_R16_SNORM;
            case TextureFormat::R16_UINT:           return VK_FORMAT_R16_UINT;
            case TextureFormat::R16_SINT:           return VK_FORMAT_R16_SINT;
            case TextureFormat::R16_SFLOAT:         return VK_FORMAT_R16_SFLOAT;
            case TextureFormat::R16G16_UNORM:       return VK_FORMAT_R16G16_UNORM;
            case TextureFormat::R16G16_SNORM:       return VK_FORMAT_R16G16_SNORM;
            case TextureFormat::R16G16_UINT:        return VK_FORMAT_R16G16_UINT;
            case TextureFormat::R16G16_SINT:        return VK_FORMAT_R16G16_SINT;
            case TextureFormat::R16G16_SFLOAT:      return VK_FORMAT_R16G16_SFLOAT;
            case TextureFormat::R16G16B16A16_UNORM: return VK_FORMAT_R16G16B16A16_UNORM;
            case TextureFormat::R16G16B16A16_SNORM: return VK_FORMAT_R16G16B16A16_SNORM;
            case TextureFormat::R16G16B16A16_UINT:  return VK_FORMAT_R16G16B16A16_UINT;
            case TextureFormat::R16G16B16A16_SINT:  return VK_FORMAT_R16G16B16A16_SINT;
            case TextureFormat::R16G16B16A16_SFLOAT:return VK_FORMAT_R16G16B16A16_SFLOAT;
            case TextureFormat::R32_UINT:           return VK_FORMAT_R32_UINT;
            case TextureFormat::R32_SINT:           return VK_FORMAT_R32_SINT;
            case TextureFormat::R32_SFLOAT:         return VK_FORMAT_R32_SFLOAT;
            case TextureFormat::R32G32_UINT:        return VK_FORMAT_R32G32_UINT;
            case TextureFormat::R32G32_SINT:        return VK_FORMAT_R32G32_SINT;
            case TextureFormat::R32G32_SFLOAT:      return VK_FORMAT_R32G32_SFLOAT;
            case TextureFormat::R32G32B32_UINT:     return VK_FORMAT_R32G32B32_UINT;
            case TextureFormat::R32G32B32_SINT:     return VK_FORMAT_R32G32B32_SINT;
            case TextureFormat::R32G32B32_SFLOAT:   return VK_FORMAT_R32G32B32_SFLOAT;
            case TextureFormat::R32G32B32A32_UINT:  return VK_FORMAT_R32G32B32A32_UINT;
            case TextureFormat::R32G32B32A32_SINT:  return VK_FORMAT_R32G32B32A32_SINT;
            case TextureFormat::R32G32B32A32_SFLOAT:return VK_FORMAT_R32G32B32A32_SFLOAT;
            case TextureFormat::D16_UNORM:          return VK_FORMAT_D16_UNORM;
            case TextureFormat::D32_SFLOAT:         return VK_FORMAT_D32_SFLOAT;
            case TextureFormat::D24_UNORM_S8_UINT:  return VK_FORMAT_D24_UNORM_S8_UINT;
            case TextureFormat::D32_SFLOAT_S8_UINT: return VK_FORMAT_D32_SFLOAT_S8_UINT;
            case TextureFormat::S8_UINT:            return VK_FORMAT_S8_UINT;
            case TextureFormat::BC1_RGB_UNORM:      return VK_FORMAT_BC1_RGB_UNORM_BLOCK;
            case TextureFormat::BC1_RGB_SRGB:       return VK_FORMAT_BC1_RGB_SRGB_BLOCK;
            case TextureFormat::BC1_RGBA_UNORM:     return VK_FORMAT_BC1_RGBA_UNORM_BLOCK;
            case TextureFormat::BC1_RGBA_SRGB:      return VK_FORMAT_BC1_RGBA_SRGB_BLOCK;
            case TextureFormat::BC2_UNORM:          return VK_FORMAT_BC2_UNORM_BLOCK;
            case TextureFormat::BC2_SRGB:           return VK_FORMAT_BC2_SRGB_BLOCK;
            case TextureFormat::BC3_UNORM:          return VK_FORMAT_BC3_UNORM_BLOCK;
            case TextureFormat::BC3_SRGB:           return VK_FORMAT_BC3_SRGB_BLOCK;
            case TextureFormat::BC4_UNORM:          return VK_FORMAT_BC4_UNORM_BLOCK;
            case TextureFormat::BC4_SNORM:          return VK_FORMAT_BC4_SNORM_BLOCK;
            case TextureFormat::BC5_UNORM:          return VK_FORMAT_BC5_UNORM_BLOCK;
            case TextureFormat::BC5_SNORM:          return VK_FORMAT_BC5_SNORM_BLOCK;
            case TextureFormat::BC6H_UFLOAT:        return VK_FORMAT_BC6H_UFLOAT_BLOCK;
            case TextureFormat::BC6H_SFLOAT:        return VK_FORMAT_BC6H_SFLOAT_BLOCK;
            case TextureFormat::BC7_UNORM:          return VK_FORMAT_BC7_UNORM_BLOCK;
            case TextureFormat::BC7_SRGB:           return VK_FORMAT_BC7_SRGB_BLOCK;
            default:                                return VK_FORMAT_UNDEFINED;
        }
    }

    VkFilter ToVulkan(Filter filter)
    {
        switch (filter)
        {
            case Filter::Nearest: return VK_FILTER_NEAREST;
            case Filter::Linear:  return VK_FILTER_LINEAR;
            default:              return VK_FILTER_LINEAR;
        }
    }

    VkSamplerMipmapMode ToVulkan(MipmapMode mode)
    {
        switch (mode)
        {
            case MipmapMode::Nearest: return VK_SAMPLER_MIPMAP_MODE_NEAREST;
            case MipmapMode::Linear:  return VK_SAMPLER_MIPMAP_MODE_LINEAR;
            default:                  return VK_SAMPLER_MIPMAP_MODE_LINEAR;
        }
    }

    VkSamplerAddressMode ToVulkan(AddressMode mode)
    {
        switch (mode)
        {
            case AddressMode::Repeat:           return VK_SAMPLER_ADDRESS_MODE_REPEAT;
            case AddressMode::MirroredRepeat:   return VK_SAMPLER_ADDRESS_MODE_MIRRORED_REPEAT;
            case AddressMode::ClampToEdge:      return VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
            case AddressMode::ClampToBorder:    return VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_BORDER;
            case AddressMode::MirrorClampToEdge:return VK_SAMPLER_ADDRESS_MODE_MIRROR_CLAMP_TO_EDGE;
            default:                            return VK_SAMPLER_ADDRESS_MODE_REPEAT;
        }
    }

    VkBorderColor ToVulkan(BorderColor color)
    {
        switch (color)
        {
            case BorderColor::FloatTransparentBlack: return VK_BORDER_COLOR_FLOAT_TRANSPARENT_BLACK;
            case BorderColor::IntTransparentBlack:   return VK_BORDER_COLOR_INT_TRANSPARENT_BLACK;
            case BorderColor::FloatOpaqueBlack:      return VK_BORDER_COLOR_FLOAT_OPAQUE_BLACK;
            case BorderColor::IntOpaqueBlack:        return VK_BORDER_COLOR_INT_OPAQUE_BLACK;
            case BorderColor::FloatOpaqueWhite:      return VK_BORDER_COLOR_FLOAT_OPAQUE_WHITE;
            case BorderColor::IntOpaqueWhite:        return VK_BORDER_COLOR_INT_OPAQUE_WHITE;
            default:                                 return VK_BORDER_COLOR_FLOAT_TRANSPARENT_BLACK;
        }
    }

    VkIndexType ToVulkan(IndexType type)
    {
        switch (type)
        {
            case IndexType::UInt16: return VK_INDEX_TYPE_UINT16;
            case IndexType::UInt32: return VK_INDEX_TYPE_UINT32;
            default:                return VK_INDEX_TYPE_UINT32;
        }
    }

    VkBlendFactor ToVulkan(BlendFactor factor)
    {
        switch (factor)
        {
            case BlendFactor::Zero:                  return VK_BLEND_FACTOR_ZERO;
            case BlendFactor::One:                   return VK_BLEND_FACTOR_ONE;
            case BlendFactor::SrcColor:              return VK_BLEND_FACTOR_SRC_COLOR;
            case BlendFactor::OneMinusSrcColor:      return VK_BLEND_FACTOR_ONE_MINUS_SRC_COLOR;
            case BlendFactor::DstColor:              return VK_BLEND_FACTOR_DST_COLOR;
            case BlendFactor::OneMinusDstColor:      return VK_BLEND_FACTOR_ONE_MINUS_DST_COLOR;
            case BlendFactor::SrcAlpha:              return VK_BLEND_FACTOR_SRC_ALPHA;
            case BlendFactor::OneMinusSrcAlpha:      return VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA;
            case BlendFactor::DstAlpha:              return VK_BLEND_FACTOR_DST_ALPHA;
            case BlendFactor::OneMinusDstAlpha:      return VK_BLEND_FACTOR_ONE_MINUS_DST_ALPHA;
            case BlendFactor::ConstantColor:         return VK_BLEND_FACTOR_CONSTANT_COLOR;
            case BlendFactor::OneMinusConstantColor: return VK_BLEND_FACTOR_ONE_MINUS_CONSTANT_COLOR;
            case BlendFactor::ConstantAlpha:         return VK_BLEND_FACTOR_CONSTANT_ALPHA;
            case BlendFactor::OneMinusConstantAlpha: return VK_BLEND_FACTOR_ONE_MINUS_CONSTANT_ALPHA;
            case BlendFactor::SrcAlphaSaturate:      return VK_BLEND_FACTOR_SRC_ALPHA_SATURATE;
            default:                                 return VK_BLEND_FACTOR_ONE;
        }
    }

    VkBlendOp ToVulkan(BlendOp op)
    {
        switch (op)
        {
            case BlendOp::Add:             return VK_BLEND_OP_ADD;
            case BlendOp::Subtract:        return VK_BLEND_OP_SUBTRACT;
            case BlendOp::ReverseSubtract: return VK_BLEND_OP_REVERSE_SUBTRACT;
            case BlendOp::Min:             return VK_BLEND_OP_MIN;
            case BlendOp::Max:             return VK_BLEND_OP_MAX;
            default:                       return VK_BLEND_OP_ADD;
        }
    }

    VkColorComponentFlags ToVulkan(ColorComponent components)
    {
        VkColorComponentFlags flags = 0;
        if ((static_cast<uint8_t>(components) & static_cast<uint8_t>(ColorComponent::R)) != 0)
            flags |= VK_COLOR_COMPONENT_R_BIT;
        if ((static_cast<uint8_t>(components) & static_cast<uint8_t>(ColorComponent::G)) != 0)
            flags |= VK_COLOR_COMPONENT_G_BIT;
        if ((static_cast<uint8_t>(components) & static_cast<uint8_t>(ColorComponent::B)) != 0)
            flags |= VK_COLOR_COMPONENT_B_BIT;
        if ((static_cast<uint8_t>(components) & static_cast<uint8_t>(ColorComponent::A)) != 0)
            flags |= VK_COLOR_COMPONENT_A_BIT;
        return flags;
    }

    VkAttachmentLoadOp ToVulkan(LoadOp op)
    {
        switch (op)
        {
            case LoadOp::Load:     return VK_ATTACHMENT_LOAD_OP_LOAD;
            case LoadOp::Clear:    return VK_ATTACHMENT_LOAD_OP_CLEAR;
            case LoadOp::DontCare: return VK_ATTACHMENT_LOAD_OP_DONT_CARE;
            default:               return VK_ATTACHMENT_LOAD_OP_CLEAR;
        }
    }

    VkAttachmentStoreOp ToVulkan(StoreOp op)
    {
        switch (op)
        {
            case StoreOp::Store:    return VK_ATTACHMENT_STORE_OP_STORE;
            case StoreOp::DontCare: return VK_ATTACHMENT_STORE_OP_DONT_CARE;
            default:                return VK_ATTACHMENT_STORE_OP_STORE;
        }
    }

    VkImageLayout ToVulkan(ImageLayout layout)
    {
        switch (layout)
        {
            case ImageLayout::Undefined:              return VK_IMAGE_LAYOUT_UNDEFINED;
            case ImageLayout::General:                return VK_IMAGE_LAYOUT_GENERAL;
            case ImageLayout::ColorAttachment:        return VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
            case ImageLayout::DepthStencilAttachment: return VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
            case ImageLayout::DepthStencilReadOnly:   return VK_IMAGE_LAYOUT_DEPTH_STENCIL_READ_ONLY_OPTIMAL;
            case ImageLayout::ShaderReadOnly:         return VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
            case ImageLayout::TransferSrc:            return VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
            case ImageLayout::TransferDst:            return VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            case ImageLayout::Present:                return VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
            default:                                  return VK_IMAGE_LAYOUT_UNDEFINED;
        }
    }

    // Buffer usage conversion

    VkBufferUsageFlags ToVulkanBufferUsage(BufferUsage usage, bool cpuVisible)
    {
        VkBufferUsageFlags flags = 0;

        switch (usage)
        {
            case BufferUsage::Vertex:
                flags = VK_BUFFER_USAGE_VERTEX_BUFFER_BIT;
                break;
            case BufferUsage::Index:
                flags = VK_BUFFER_USAGE_INDEX_BUFFER_BIT;
                break;
            case BufferUsage::Uniform:
                flags = VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT;
                break;
            case BufferUsage::Storage:
                flags = VK_BUFFER_USAGE_STORAGE_BUFFER_BIT;
                break;
            case BufferUsage::Staging:
                flags = VK_BUFFER_USAGE_TRANSFER_SRC_BIT;
                break;
            case BufferUsage::Indirect:
                flags = VK_BUFFER_USAGE_INDIRECT_BUFFER_BIT;
                break;
        }

        // Non-staging, non-CPU-visible buffers need transfer dst for data uploads
        if (usage != BufferUsage::Staging && !cpuVisible)
            flags |= VK_BUFFER_USAGE_TRANSFER_DST_BIT;

        return flags;
    }

    // Reverse conversions

    TextureFormat FromVulkan(VkFormat format)
    {
        switch (format)
        {
            case VK_FORMAT_UNDEFINED:            return TextureFormat::Undefined;
            case VK_FORMAT_R8_UNORM:             return TextureFormat::R8_UNORM;
            case VK_FORMAT_R8G8_UNORM:           return TextureFormat::R8G8_UNORM;
            case VK_FORMAT_R8G8B8A8_UNORM:       return TextureFormat::R8G8B8A8_UNORM;
            case VK_FORMAT_R8G8B8A8_SRGB:        return TextureFormat::R8G8B8A8_SRGB;
            case VK_FORMAT_B8G8R8A8_UNORM:       return TextureFormat::B8G8R8A8_UNORM;
            case VK_FORMAT_B8G8R8A8_SRGB:        return TextureFormat::B8G8R8A8_SRGB;
            case VK_FORMAT_R16G16B16A16_SFLOAT:  return TextureFormat::R16G16B16A16_SFLOAT;
            case VK_FORMAT_R32_SFLOAT:           return TextureFormat::R32_SFLOAT;
            case VK_FORMAT_R32G32_SFLOAT:        return TextureFormat::R32G32_SFLOAT;
            case VK_FORMAT_R32G32B32_SFLOAT:     return TextureFormat::R32G32B32_SFLOAT;
            case VK_FORMAT_R32G32B32A32_SFLOAT:  return TextureFormat::R32G32B32A32_SFLOAT;
            case VK_FORMAT_D16_UNORM:            return TextureFormat::D16_UNORM;
            case VK_FORMAT_D32_SFLOAT:           return TextureFormat::D32_SFLOAT;
            case VK_FORMAT_D24_UNORM_S8_UINT:    return TextureFormat::D24_UNORM_S8_UINT;
            case VK_FORMAT_D32_SFLOAT_S8_UINT:   return TextureFormat::D32_SFLOAT_S8_UINT;
            default:                             return TextureFormat::Undefined;
        }
    }

    ShaderStage FromVulkanShaderStage(VkShaderStageFlagBits stage)
    {
        switch (stage)
        {
            case VK_SHADER_STAGE_VERTEX_BIT:                  return ShaderStage::Vertex;
            case VK_SHADER_STAGE_TESSELLATION_CONTROL_BIT:    return ShaderStage::TessellationControl;
            case VK_SHADER_STAGE_TESSELLATION_EVALUATION_BIT: return ShaderStage::TessellationEvaluation;
            case VK_SHADER_STAGE_GEOMETRY_BIT:                return ShaderStage::Geometry;
            case VK_SHADER_STAGE_FRAGMENT_BIT:                return ShaderStage::Fragment;
            case VK_SHADER_STAGE_COMPUTE_BIT:                 return ShaderStage::Compute;
            default:                                          return ShaderStage::None;
        }
    }

    // Vertex input conversions

    VkVertexInputRate ToVulkan(VertexInputRate rate)
    {
        switch (rate)
        {
            case VertexInputRate::Vertex:   return VK_VERTEX_INPUT_RATE_VERTEX;
            case VertexInputRate::Instance: return VK_VERTEX_INPUT_RATE_INSTANCE;
            default:                        return VK_VERTEX_INPUT_RATE_VERTEX;
        }
    }

    VkVertexInputBindingDescription ToVulkan(const RHIVertexBindingDescription& desc)
    {
        VkVertexInputBindingDescription vkDesc{};
        vkDesc.binding = desc.binding;
        vkDesc.stride = desc.stride;
        vkDesc.inputRate = ToVulkan(desc.inputRate);
        return vkDesc;
    }

    VkVertexInputAttributeDescription ToVulkan(const RHIVertexAttributeDescription& desc)
    {
        VkVertexInputAttributeDescription vkDesc{};
        vkDesc.location = desc.location;
        vkDesc.binding = desc.binding;
        vkDesc.format = ToVulkan(desc.format);
        vkDesc.offset = desc.offset;
        return vkDesc;
    }

}
