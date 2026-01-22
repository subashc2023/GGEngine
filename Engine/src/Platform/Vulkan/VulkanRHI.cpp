#include "ggpch.h"
#include "VulkanRHI.h"
#include "VulkanContext.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/RHI/RHICommandBuffer.h"

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

    // ============================================================================
    // Vulkan Resource Registry Implementation
    // ============================================================================

    VulkanResourceRegistry& VulkanResourceRegistry::Get()
    {
        static VulkanResourceRegistry instance;
        return instance;
    }

    uint64_t VulkanResourceRegistry::GenerateId()
    {
        return m_NextId++;
    }

    // Pipeline
    RHIPipelineHandle VulkanResourceRegistry::RegisterPipeline(VkPipeline pipeline, VkPipelineLayout layout)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Pipelines[id] = { pipeline, layout };
        return RHIPipelineHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterPipeline(RHIPipelineHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Pipelines.erase(handle.id);
    }

    VulkanResourceRegistry::PipelineData VulkanResourceRegistry::GetPipelineData(RHIPipelineHandle handle) const
    {
        auto it = m_Pipelines.find(handle.id);
        if (it != m_Pipelines.end())
            return it->second;
        return {};
    }

    VkPipeline VulkanResourceRegistry::GetPipeline(RHIPipelineHandle handle) const
    {
        return GetPipelineData(handle).pipeline;
    }

    VkPipelineLayout VulkanResourceRegistry::GetPipelineLayout(RHIPipelineHandle handle) const
    {
        return GetPipelineData(handle).layout;
    }

    // Pipeline Layout
    RHIPipelineLayoutHandle VulkanResourceRegistry::RegisterPipelineLayout(VkPipelineLayout layout)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_PipelineLayouts[id] = layout;
        return RHIPipelineLayoutHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterPipelineLayout(RHIPipelineLayoutHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_PipelineLayouts.erase(handle.id);
    }

    VkPipelineLayout VulkanResourceRegistry::GetPipelineLayout(RHIPipelineLayoutHandle handle) const
    {
        auto it = m_PipelineLayouts.find(handle.id);
        if (it != m_PipelineLayouts.end())
            return it->second;
        return VK_NULL_HANDLE;
    }

    // Render Pass
    RHIRenderPassHandle VulkanResourceRegistry::RegisterRenderPass(VkRenderPass renderPass, VkFramebuffer framebuffer,
                                                                    uint32_t width, uint32_t height)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

        // Check if this render pass is already registered (idempotent)
        for (const auto& [id, data] : m_RenderPasses)
        {
            if (data.renderPass == renderPass)
            {
                // Update framebuffer info if provided (may change on resize)
                if (framebuffer != VK_NULL_HANDLE)
                {
                    m_RenderPasses[id].framebuffer = framebuffer;
                    m_RenderPasses[id].width = width;
                    m_RenderPasses[id].height = height;
                }
                return RHIRenderPassHandle{ id };
            }
        }

        // Not found, register new
        uint64_t id = GenerateId();
        m_RenderPasses[id] = { renderPass, framebuffer, width, height };
        return RHIRenderPassHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterRenderPass(RHIRenderPassHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_RenderPasses.erase(handle.id);
    }

    VulkanResourceRegistry::RenderPassData VulkanResourceRegistry::GetRenderPassData(RHIRenderPassHandle handle) const
    {
        auto it = m_RenderPasses.find(handle.id);
        if (it != m_RenderPasses.end())
            return it->second;
        return {};
    }

    VkRenderPass VulkanResourceRegistry::GetRenderPass(RHIRenderPassHandle handle) const
    {
        return GetRenderPassData(handle).renderPass;
    }

    VkFramebuffer VulkanResourceRegistry::GetFramebuffer(RHIRenderPassHandle handle) const
    {
        return GetRenderPassData(handle).framebuffer;
    }

    // Buffer
    RHIBufferHandle VulkanResourceRegistry::RegisterBuffer(VkBuffer buffer, VmaAllocation allocation,
                                                           uint64_t size, bool cpuVisible)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Buffers[id] = { buffer, allocation, size, cpuVisible };
        return RHIBufferHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterBuffer(RHIBufferHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Buffers.erase(handle.id);
    }

    VulkanResourceRegistry::BufferData VulkanResourceRegistry::GetBufferData(RHIBufferHandle handle) const
    {
        auto it = m_Buffers.find(handle.id);
        if (it != m_Buffers.end())
            return it->second;
        return {};
    }

    VkBuffer VulkanResourceRegistry::GetBuffer(RHIBufferHandle handle) const
    {
        return GetBufferData(handle).buffer;
    }

    VmaAllocation VulkanResourceRegistry::GetBufferAllocation(RHIBufferHandle handle) const
    {
        return GetBufferData(handle).allocation;
    }

    // Texture
    RHITextureHandle VulkanResourceRegistry::RegisterTexture(VkImage image, VkImageView view, VkSampler sampler,
                                                             VmaAllocation allocation, uint32_t width, uint32_t height,
                                                             TextureFormat format)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Textures[id] = { image, view, sampler, allocation, width, height, format };
        return RHITextureHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterTexture(RHITextureHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Textures.erase(handle.id);
    }

    VulkanResourceRegistry::TextureData VulkanResourceRegistry::GetTextureData(RHITextureHandle handle) const
    {
        auto it = m_Textures.find(handle.id);
        if (it != m_Textures.end())
            return it->second;
        return {};
    }

    VkImage VulkanResourceRegistry::GetTextureImage(RHITextureHandle handle) const
    {
        return GetTextureData(handle).image;
    }

    VkImageView VulkanResourceRegistry::GetTextureView(RHITextureHandle handle) const
    {
        return GetTextureData(handle).imageView;
    }

    VkSampler VulkanResourceRegistry::GetTextureSampler(RHITextureHandle handle) const
    {
        return GetTextureData(handle).sampler;
    }

    // Shader Module (individual stages)
    RHIShaderModuleHandle VulkanResourceRegistry::RegisterShaderModule(VkShaderModule module, ShaderStage stage, const std::string& entryPoint)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_ShaderModules[id] = { module, stage, entryPoint };
        return RHIShaderModuleHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterShaderModule(RHIShaderModuleHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_ShaderModules.erase(handle.id);
    }

    VulkanResourceRegistry::ShaderModuleData VulkanResourceRegistry::GetShaderModuleData(RHIShaderModuleHandle handle) const
    {
        auto it = m_ShaderModules.find(handle.id);
        if (it != m_ShaderModules.end())
            return it->second;
        return {};
    }

    VkShaderModule VulkanResourceRegistry::GetShaderModule(RHIShaderModuleHandle handle) const
    {
        return GetShaderModuleData(handle).module;
    }

    // Shader Program (collection of modules)
    RHIShaderHandle VulkanResourceRegistry::RegisterShader(const std::vector<RHIShaderModuleHandle>& moduleHandles)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Shaders[id] = { moduleHandles };
        return RHIShaderHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterShader(RHIShaderHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Shaders.erase(handle.id);
    }

    VulkanResourceRegistry::ShaderData VulkanResourceRegistry::GetShaderData(RHIShaderHandle handle) const
    {
        auto it = m_Shaders.find(handle.id);
        if (it != m_Shaders.end())
            return it->second;
        return {};
    }

    std::vector<VkPipelineShaderStageCreateInfo> VulkanResourceRegistry::GetShaderPipelineStageCreateInfos(RHIShaderHandle handle) const
    {
        std::vector<VkPipelineShaderStageCreateInfo> infos;
        auto data = GetShaderData(handle);
        infos.reserve(data.moduleHandles.size());

        for (const auto& moduleHandle : data.moduleHandles)
        {
            auto moduleData = GetShaderModuleData(moduleHandle);
            if (moduleData.module == VK_NULL_HANDLE)
                continue;

            VkPipelineShaderStageCreateInfo info{};
            info.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
            info.stage = static_cast<VkShaderStageFlagBits>(ToVulkan(moduleData.stage));
            info.module = moduleData.module;
            info.pName = moduleData.entryPoint.c_str();
            infos.push_back(info);
        }

        return infos;
    }

    // Descriptor Set Layout
    RHIDescriptorSetLayoutHandle VulkanResourceRegistry::RegisterDescriptorSetLayout(VkDescriptorSetLayout layout)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_DescriptorSetLayouts[id] = layout;
        return RHIDescriptorSetLayoutHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_DescriptorSetLayouts.erase(handle.id);
    }

    VkDescriptorSetLayout VulkanResourceRegistry::GetDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle) const
    {
        auto it = m_DescriptorSetLayouts.find(handle.id);
        if (it != m_DescriptorSetLayouts.end())
            return it->second;
        return VK_NULL_HANDLE;
    }

    // Descriptor Set
    RHIDescriptorSetHandle VulkanResourceRegistry::RegisterDescriptorSet(VkDescriptorSet set,
                                                                          RHIDescriptorSetLayoutHandle layoutHandle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_DescriptorSets[id] = { set, layoutHandle };
        return RHIDescriptorSetHandle{ id };
    }

    void VulkanResourceRegistry::UnregisterDescriptorSet(RHIDescriptorSetHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_DescriptorSets.erase(handle.id);
    }

    VulkanResourceRegistry::DescriptorSetData VulkanResourceRegistry::GetDescriptorSetData(RHIDescriptorSetHandle handle) const
    {
        auto it = m_DescriptorSets.find(handle.id);
        if (it != m_DescriptorSets.end())
            return it->second;
        return {};
    }

    VkDescriptorSet VulkanResourceRegistry::GetDescriptorSet(RHIDescriptorSetHandle handle) const
    {
        return GetDescriptorSetData(handle).descriptorSet;
    }

    // Command Buffer
    void VulkanResourceRegistry::SetCurrentCommandBuffer(uint32_t frameIndex, VkCommandBuffer cmd)
    {
        if (frameIndex < MaxFramesInFlight)
        {
            m_CommandBuffers[frameIndex] = cmd;
            // Generate a unique handle ID for this frame's command buffer
            // We use a fixed offset to distinguish command buffer handles
            m_CommandBufferHandleIds[frameIndex] = 0xFFFF0000 + frameIndex;
        }
    }

    RHICommandBufferHandle VulkanResourceRegistry::GetCurrentCommandBufferHandle(uint32_t frameIndex) const
    {
        if (frameIndex < MaxFramesInFlight)
            return RHICommandBufferHandle{ m_CommandBufferHandleIds[frameIndex] };
        return NullCommandBuffer;
    }

    VkCommandBuffer VulkanResourceRegistry::GetCommandBuffer(RHICommandBufferHandle handle) const
    {
        // Check if it's a frame command buffer
        for (uint32_t i = 0; i < MaxFramesInFlight; ++i)
        {
            if (m_CommandBufferHandleIds[i] == handle.id)
                return m_CommandBuffers[i];
        }
        return VK_NULL_HANDLE;
    }

    void VulkanResourceRegistry::Clear()
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Pipelines.clear();
        m_PipelineLayouts.clear();
        m_RenderPasses.clear();
        m_Buffers.clear();
        m_Textures.clear();
        m_ShaderModules.clear();
        m_Shaders.clear();
        m_DescriptorSetLayouts.clear();
        m_DescriptorSets.clear();
    }

    // ============================================================================
    // RHIDevice Implementation
    // ============================================================================

    RHIDevice& RHIDevice::Get()
    {
        static RHIDevice instance;
        return instance;
    }

    void RHIDevice::Init(void* windowHandle)
    {
        // RHIDevice wraps VulkanContext - the actual initialization happens there
        // This is called after VulkanContext::Init() to set up RHI-specific state
        m_Initialized = true;

        // Register the swapchain render pass
        auto& vkContext = VulkanContext::Get();
        auto& registry = VulkanResourceRegistry::Get();
        VkExtent2D extent = vkContext.GetSwapchainExtent();
        m_SwapchainRenderPassHandle = registry.RegisterRenderPass(
            vkContext.GetRenderPass(),
            VK_NULL_HANDLE,  // No fixed framebuffer - swapchain framebuffers change per-frame
            extent.width,
            extent.height
        );
    }

    void RHIDevice::Shutdown()
    {
        if (m_Initialized)
        {
            VulkanResourceRegistry::Get().Clear();
            m_Initialized = false;
        }
    }

    void RHIDevice::BeginFrame()
    {
        auto& vkContext = VulkanContext::Get();
        vkContext.BeginFrame();

        // Update command buffer handle for this frame
        auto& registry = VulkanResourceRegistry::Get();
        uint32_t frameIndex = vkContext.GetCurrentFrameIndex();
        registry.SetCurrentCommandBuffer(frameIndex, vkContext.GetCurrentCommandBuffer());
    }

    void RHIDevice::EndFrame()
    {
        VulkanContext::Get().EndFrame();
    }

    void RHIDevice::BeginSwapchainRenderPass()
    {
        VulkanContext::Get().BeginSwapchainRenderPass();
    }

    RHICommandBufferHandle RHIDevice::GetCurrentCommandBuffer() const
    {
        uint32_t frameIndex = VulkanContext::Get().GetCurrentFrameIndex();
        return VulkanResourceRegistry::Get().GetCurrentCommandBufferHandle(frameIndex);
    }

    RHIRenderPassHandle RHIDevice::GetSwapchainRenderPass() const
    {
        return m_SwapchainRenderPassHandle;
    }

    uint32_t RHIDevice::GetSwapchainWidth() const
    {
        return VulkanContext::Get().GetSwapchainExtent().width;
    }

    uint32_t RHIDevice::GetSwapchainHeight() const
    {
        return VulkanContext::Get().GetSwapchainExtent().height;
    }

    uint32_t RHIDevice::GetCurrentFrameIndex() const
    {
        return VulkanContext::Get().GetCurrentFrameIndex();
    }

    void RHIDevice::ImmediateSubmit(const std::function<void(RHICommandBufferHandle)>& func)
    {
        VulkanContext::Get().ImmediateSubmit([&func](VkCommandBuffer vkCmd) {
            // Create a temporary handle for immediate commands
            // We use a special ID that won't conflict with frame command buffers
            RHICommandBufferHandle tempHandle{0xFFFE0000};
            // Store temporarily in registry
            auto& registry = VulkanResourceRegistry::Get();
            registry.SetCurrentCommandBuffer(0, vkCmd);  // Temporarily store in slot 0
            RHICommandBufferHandle handle = registry.GetCurrentCommandBufferHandle(0);
            func(handle);
        });
    }

    void RHIDevice::WaitIdle()
    {
        vkDeviceWaitIdle(VulkanContext::Get().GetDevice());
    }

    void RHIDevice::OnWindowResize(uint32_t width, uint32_t height)
    {
        VulkanContext::Get().OnWindowResize(width, height);
    }

    void RHIDevice::SetVSync(bool enabled)
    {
        VulkanContext::Get().SetVSync(enabled);
    }

    bool RHIDevice::IsVSync() const
    {
        return VulkanContext::Get().IsVSync();
    }

    RHIDescriptorSetLayoutHandle RHIDevice::CreateDescriptorSetLayout(const std::vector<RHIDescriptorBinding>& bindings)
    {
        auto device = VulkanContext::Get().GetDevice();

        std::vector<VkDescriptorSetLayoutBinding> vkBindings;
        vkBindings.reserve(bindings.size());

        for (const auto& binding : bindings)
        {
            VkDescriptorSetLayoutBinding vkBinding{};
            vkBinding.binding = binding.binding;
            vkBinding.descriptorType = ToVulkan(binding.type);
            vkBinding.descriptorCount = binding.count;
            vkBinding.stageFlags = ToVulkan(binding.stages);
            vkBinding.pImmutableSamplers = nullptr;
            vkBindings.push_back(vkBinding);
        }

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.bindingCount = static_cast<uint32_t>(vkBindings.size());
        layoutInfo.pBindings = vkBindings.data();

        VkDescriptorSetLayout layout;
        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create descriptor set layout!");
            return NullDescriptorSetLayout;
        }

        return VulkanResourceRegistry::Get().RegisterDescriptorSetLayout(layout);
    }

    void RHIDevice::DestroyDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle)
    {
        if (!handle.IsValid()) return;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(handle);
        if (layout != VK_NULL_HANDLE)
        {
            vkDestroyDescriptorSetLayout(device, layout, nullptr);
        }
        registry.UnregisterDescriptorSetLayout(handle);
    }

    RHIDescriptorSetHandle RHIDevice::AllocateDescriptorSet(RHIDescriptorSetLayoutHandle layoutHandle)
    {
        if (!layoutHandle.IsValid()) return NullDescriptorSet;

        auto device = VulkanContext::Get().GetDevice();
        auto pool = VulkanContext::Get().GetDescriptorPool();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(layoutHandle);

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &layout;

        VkDescriptorSet descriptorSet;
        if (vkAllocateDescriptorSets(device, &allocInfo, &descriptorSet) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to allocate descriptor set!");
            return NullDescriptorSet;
        }

        return registry.RegisterDescriptorSet(descriptorSet, layoutHandle);
    }

    void RHIDevice::FreeDescriptorSet(RHIDescriptorSetHandle handle)
    {
        if (!handle.IsValid()) return;

        auto device = VulkanContext::Get().GetDevice();
        auto pool = VulkanContext::Get().GetDescriptorPool();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSet descriptorSet = registry.GetDescriptorSet(handle);

        if (descriptorSet != VK_NULL_HANDLE)
        {
            vkFreeDescriptorSets(device, pool, 1, &descriptorSet);
        }
        registry.UnregisterDescriptorSet(handle);
    }

    // ============================================================================
    // RHICmd Implementation
    // ============================================================================

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, float x, float y, float width, float height,
                             float minDepth, float maxDepth)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        VkViewport viewport{};
        viewport.x = x;
        viewport.y = height;  // Flip Y for Vulkan coordinate system
        viewport.width = width;
        viewport.height = -height;  // Negative height for Y-flip
        viewport.minDepth = minDepth;
        viewport.maxDepth = maxDepth;
        vkCmdSetViewport(vkCmd, 0, 1, &viewport);
    }

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, float width, float height)
    {
        SetViewport(cmd, 0.0f, 0.0f, width, height, 0.0f, 1.0f);
    }

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        SetViewport(cmd, 0.0f, 0.0f, static_cast<float>(width), static_cast<float>(height), 0.0f, 1.0f);
    }

    void RHICmd::SetScissor(RHICommandBufferHandle cmd, int32_t x, int32_t y, uint32_t width, uint32_t height)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        VkRect2D scissor{};
        scissor.offset = { x, y };
        scissor.extent = { width, height };
        vkCmdSetScissor(vkCmd, 0, 1, &scissor);
    }

    void RHICmd::SetScissor(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        SetScissor(cmd, 0, 0, width, height);
    }

    void RHICmd::BindPipeline(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipeline vkPipeline = registry.GetPipeline(pipeline);
        if (vkCmd == VK_NULL_HANDLE || vkPipeline == VK_NULL_HANDLE) return;

        vkCmdBindPipeline(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, vkPipeline);
    }

    void RHICmd::BindVertexBuffer(RHICommandBufferHandle cmd, RHIBufferHandle buffer, uint32_t binding)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer vkBuffer = registry.GetBuffer(buffer);
        if (vkCmd == VK_NULL_HANDLE || vkBuffer == VK_NULL_HANDLE) return;

        VkDeviceSize offset = 0;
        vkCmdBindVertexBuffers(vkCmd, binding, 1, &vkBuffer, &offset);
    }

    void RHICmd::BindIndexBuffer(RHICommandBufferHandle cmd, RHIBufferHandle buffer, IndexType indexType)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer vkBuffer = registry.GetBuffer(buffer);
        if (vkCmd == VK_NULL_HANDLE || vkBuffer == VK_NULL_HANDLE) return;

        vkCmdBindIndexBuffer(vkCmd, vkBuffer, 0, ToVulkan(indexType));
    }

    void RHICmd::BindDescriptorSet(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                                   RHIDescriptorSetHandle set, uint32_t setIndex)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout layout = registry.GetPipelineLayout(pipeline);
        VkDescriptorSet vkSet = registry.GetDescriptorSet(set);
        if (vkCmd == VK_NULL_HANDLE || layout == VK_NULL_HANDLE || vkSet == VK_NULL_HANDLE) return;

        vkCmdBindDescriptorSets(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, layout, setIndex, 1, &vkSet, 0, nullptr);
    }

    void RHICmd::PushConstants(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                               ShaderStage stages, uint32_t offset, uint32_t size, const void* data)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout layout = registry.GetPipelineLayout(pipeline);
        if (vkCmd == VK_NULL_HANDLE || layout == VK_NULL_HANDLE) return;

        vkCmdPushConstants(vkCmd, layout, ToVulkan(stages), offset, size, data);
    }

    void RHICmd::Draw(RHICommandBufferHandle cmd, uint32_t vertexCount, uint32_t instanceCount,
                      uint32_t firstVertex, uint32_t firstInstance)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        vkCmdDraw(vkCmd, vertexCount, instanceCount, firstVertex, firstInstance);
    }

    void RHICmd::DrawIndexed(RHICommandBufferHandle cmd, uint32_t indexCount, uint32_t instanceCount,
                             uint32_t firstIndex, int32_t vertexOffset, uint32_t firstInstance)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        vkCmdDrawIndexed(vkCmd, indexCount, instanceCount, firstIndex, vertexOffset, firstInstance);
    }

    void RHICmd::BeginRenderPass(RHICommandBufferHandle cmd, RHIRenderPassHandle renderPass,
                                 RHIFramebufferHandle framebuffer, uint32_t width, uint32_t height,
                                 float clearR, float clearG, float clearB, float clearA)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        auto rpData = registry.GetRenderPassData(renderPass);
        if (vkCmd == VK_NULL_HANDLE || rpData.renderPass == VK_NULL_HANDLE) return;

        VkRenderPassBeginInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO;
        renderPassInfo.renderPass = rpData.renderPass;
        renderPassInfo.framebuffer = rpData.framebuffer;  // Use framebuffer from render pass data
        renderPassInfo.renderArea.offset = { 0, 0 };
        renderPassInfo.renderArea.extent = { width, height };

        VkClearValue clearValue{};
        clearValue.color = {{ clearR, clearG, clearB, clearA }};
        renderPassInfo.clearValueCount = 1;
        renderPassInfo.pClearValues = &clearValue;

        vkCmdBeginRenderPass(vkCmd, &renderPassInfo, VK_SUBPASS_CONTENTS_INLINE);
    }

    void RHICmd::EndRenderPass(RHICommandBufferHandle cmd)
    {
        VkCommandBuffer vkCmd = VulkanResourceRegistry::Get().GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        vkCmdEndRenderPass(vkCmd);
    }

}
