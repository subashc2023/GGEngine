#include "ggpch.h"
#include "VulkanRHI.h"
#include "VulkanContext.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/RHI/RHICommandBuffer.h"
#include <backends/imgui_impl_vulkan.h>

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
                                                                          RHIDescriptorSetLayoutHandle layoutHandle,
                                                                          VkDescriptorPool owningPool)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_DescriptorSets[id] = { set, layoutHandle, owningPool };
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
        // Check if it's the immediate command buffer
        if (handle.id == ImmediateCommandBufferHandleId)
            return m_ImmediateCommandBuffer;

        // Check if it's a frame command buffer
        for (uint32_t i = 0; i < MaxFramesInFlight; ++i)
        {
            if (m_CommandBufferHandleIds[i] == handle.id)
                return m_CommandBuffers[i];
        }
        return VK_NULL_HANDLE;
    }

    void VulkanResourceRegistry::SetImmediateCommandBuffer(VkCommandBuffer cmd)
    {
        m_ImmediateCommandBuffer = cmd;
    }

    RHICommandBufferHandle VulkanResourceRegistry::GetImmediateCommandBufferHandle() const
    {
        return RHICommandBufferHandle{ ImmediateCommandBufferHandleId };
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
            // Use the dedicated immediate command buffer slot (separate from frame buffers)
            auto& registry = VulkanResourceRegistry::Get();
            registry.SetImmediateCommandBuffer(vkCmd);
            RHICommandBufferHandle handle = registry.GetImmediateCommandBufferHandle();
            func(handle);
            registry.SetImmediateCommandBuffer(VK_NULL_HANDLE);  // Clear after use
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
        auto& registry = VulkanResourceRegistry::Get();
        auto setData = registry.GetDescriptorSetData(handle);

        if (setData.descriptorSet != VK_NULL_HANDLE)
        {
            // Use the owning pool if this set has one, otherwise use the global pool
            VkDescriptorPool pool = setData.owningPool != VK_NULL_HANDLE
                ? setData.owningPool
                : VulkanContext::Get().GetDescriptorPool();

            vkFreeDescriptorSets(device, pool, 1, &setData.descriptorSet);

            // If this set owned its pool, destroy the pool now
            if (setData.owningPool != VK_NULL_HANDLE)
            {
                vkDestroyDescriptorPool(device, setData.owningPool, nullptr);
            }
        }
        registry.UnregisterDescriptorSet(handle);
    }

    void RHIDevice::UpdateDescriptorSet(RHIDescriptorSetHandle set, const std::vector<RHIDescriptorWrite>& writes)
    {
        if (!set.IsValid() || writes.empty()) return;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSet vkSet = registry.GetDescriptorSet(set);
        if (vkSet == VK_NULL_HANDLE) return;

        std::vector<VkWriteDescriptorSet> vkWrites;
        std::vector<VkDescriptorBufferInfo> bufferInfos;
        std::vector<VkDescriptorImageInfo> imageInfos;
        bufferInfos.reserve(writes.size());
        imageInfos.reserve(writes.size());
        vkWrites.reserve(writes.size());

        for (const auto& write : writes)
        {
            VkWriteDescriptorSet vkWrite{};
            vkWrite.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
            vkWrite.dstSet = vkSet;
            vkWrite.dstBinding = write.binding;
            vkWrite.dstArrayElement = write.arrayElement;
            vkWrite.descriptorType = ToVulkan(write.type);
            vkWrite.descriptorCount = 1;

            if (std::holds_alternative<RHIDescriptorBufferInfo>(write.resource))
            {
                const auto& bufInfo = std::get<RHIDescriptorBufferInfo>(write.resource);
                auto bufferData = registry.GetBufferData(bufInfo.buffer);

                VkDescriptorBufferInfo vkBufInfo{};
                vkBufInfo.buffer = bufferData.buffer;
                vkBufInfo.offset = bufInfo.offset;
                vkBufInfo.range = bufInfo.range == 0 ? bufferData.size : bufInfo.range;
                bufferInfos.push_back(vkBufInfo);
                vkWrite.pBufferInfo = &bufferInfos.back();
            }
            else if (std::holds_alternative<RHIDescriptorImageInfo>(write.resource))
            {
                const auto& imgInfo = std::get<RHIDescriptorImageInfo>(write.resource);
                auto textureData = registry.GetTextureData(imgInfo.texture);

                VkDescriptorImageInfo vkImgInfo{};
                vkImgInfo.sampler = textureData.sampler;
                vkImgInfo.imageView = textureData.imageView;
                vkImgInfo.imageLayout = ToVulkan(imgInfo.layout);
                imageInfos.push_back(vkImgInfo);
                vkWrite.pImageInfo = &imageInfos.back();
            }

            vkWrites.push_back(vkWrite);
        }

        vkUpdateDescriptorSets(device, static_cast<uint32_t>(vkWrites.size()), vkWrites.data(), 0, nullptr);
    }

    // ============================================================================
    // Buffer Management
    // ============================================================================

    RHIBufferHandle RHIDevice::CreateBuffer(const RHIBufferSpecification& spec)
    {
        if (spec.size == 0)
        {
            GG_CORE_ERROR("RHIDevice::CreateBuffer: size is 0");
            return NullBuffer;
        }

        auto allocator = VulkanContext::Get().GetAllocator();

        VkBufferCreateInfo bufferInfo{};
        bufferInfo.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
        bufferInfo.size = spec.size;
        bufferInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;

        switch (spec.usage)
        {
            case BufferUsage::Vertex:
                bufferInfo.usage = VK_BUFFER_USAGE_VERTEX_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT;
                break;
            case BufferUsage::Index:
                bufferInfo.usage = VK_BUFFER_USAGE_INDEX_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT;
                break;
            case BufferUsage::Uniform:
                bufferInfo.usage = VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT;
                break;
            case BufferUsage::Storage:
                bufferInfo.usage = VK_BUFFER_USAGE_STORAGE_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT;
                break;
            case BufferUsage::Staging:
                bufferInfo.usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT;
                break;
            case BufferUsage::Indirect:
                bufferInfo.usage = VK_BUFFER_USAGE_INDIRECT_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT;
                break;
        }

        VmaAllocationCreateInfo allocInfo{};
        allocInfo.usage = VMA_MEMORY_USAGE_AUTO;

        if (spec.cpuVisible)
        {
            allocInfo.flags = VMA_ALLOCATION_CREATE_HOST_ACCESS_SEQUENTIAL_WRITE_BIT |
                              VMA_ALLOCATION_CREATE_MAPPED_BIT;
        }
        else
        {
            allocInfo.flags = VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT;
        }

        VkBuffer buffer = VK_NULL_HANDLE;
        VmaAllocation allocation = VK_NULL_HANDLE;

        if (vmaCreateBuffer(allocator, &bufferInfo, &allocInfo, &buffer, &allocation, nullptr) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateBuffer: vmaCreateBuffer failed");
            return NullBuffer;
        }

        return VulkanResourceRegistry::Get().RegisterBuffer(buffer, allocation, spec.size, spec.cpuVisible);
    }

    void RHIDevice::DestroyBuffer(RHIBufferHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);

        if (bufferData.buffer != VK_NULL_HANDLE)
        {
            auto allocator = VulkanContext::Get().GetAllocator();
            vmaDestroyBuffer(allocator, bufferData.buffer, bufferData.allocation);
        }

        registry.UnregisterBuffer(handle);
    }

    void* RHIDevice::MapBuffer(RHIBufferHandle handle)
    {
        if (!handle.IsValid()) return nullptr;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);
        if (!bufferData.cpuVisible) return nullptr;

        void* mapped = nullptr;
        VmaAllocator allocator = VulkanContext::Get().GetAllocator();
        if (vmaMapMemory(allocator, bufferData.allocation, &mapped) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::MapBuffer: vmaMapMemory failed");
            return nullptr;
        }
        return mapped;
    }

    void RHIDevice::UnmapBuffer(RHIBufferHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);

        VmaAllocator allocator = VulkanContext::Get().GetAllocator();
        vmaUnmapMemory(allocator, bufferData.allocation);
    }

    void RHIDevice::FlushBuffer(RHIBufferHandle handle, uint64_t offset, uint64_t size)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);

        VmaAllocator allocator = VulkanContext::Get().GetAllocator();
        vmaFlushAllocation(allocator, bufferData.allocation, offset, size == 0 ? bufferData.size : size);
    }

    void RHIDevice::UploadBufferData(RHIBufferHandle handle, const void* data, uint64_t size, uint64_t offset)
    {
        if (!handle.IsValid() || !data || size == 0) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);

        if (bufferData.cpuVisible)
        {
            void* mapped = MapBuffer(handle);
            if (mapped)
            {
                std::memcpy(static_cast<char*>(mapped) + offset, data, size);
                FlushBuffer(handle, offset, size);
                UnmapBuffer(handle);
            }
        }
        else
        {
            // Create staging buffer and copy
            RHIBufferSpecification stagingSpec;
            stagingSpec.size = size;
            stagingSpec.usage = BufferUsage::Staging;
            stagingSpec.cpuVisible = true;

            RHIBufferHandle staging = CreateBuffer(stagingSpec);
            UploadBufferData(staging, data, size, 0);

            // Copy via immediate submit
            VulkanContext::Get().ImmediateSubmit([&](VkCommandBuffer cmd) {
                VkBuffer srcBuffer = registry.GetBuffer(staging);
                VkBuffer dstBuffer = bufferData.buffer;

                VkBufferCopy copyRegion{};
                copyRegion.srcOffset = 0;
                copyRegion.dstOffset = offset;
                copyRegion.size = size;
                vkCmdCopyBuffer(cmd, srcBuffer, dstBuffer, 1, &copyRegion);
            });

            DestroyBuffer(staging);
        }
    }

    // ============================================================================
    // Texture Management
    // ============================================================================

    RHITextureHandle RHIDevice::CreateTexture(const RHITextureSpecification& spec)
    {
        auto& vkContext = VulkanContext::Get();
        auto device = vkContext.GetDevice();
        auto allocator = vkContext.GetAllocator();

        VkImageCreateInfo imageInfo{};
        imageInfo.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
        // Always use 2D for standard textures (even 1x1), only use 3D for volume textures
        imageInfo.imageType = spec.depth > 1 ? VK_IMAGE_TYPE_3D : VK_IMAGE_TYPE_2D;
        imageInfo.format = ToVulkan(spec.format);
        imageInfo.extent.width = spec.width;
        imageInfo.extent.height = spec.height;
        imageInfo.extent.depth = spec.depth;
        imageInfo.mipLevels = spec.mipLevels;
        imageInfo.arrayLayers = spec.arrayLayers;
        imageInfo.samples = ToVulkan(spec.samples);
        imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
        imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;

        // Convert usage flags
        VkImageUsageFlags vkUsage = 0;
        if (HasFlag(spec.usage, TextureUsage::Sampled))
            vkUsage |= VK_IMAGE_USAGE_SAMPLED_BIT;
        if (HasFlag(spec.usage, TextureUsage::Storage))
            vkUsage |= VK_IMAGE_USAGE_STORAGE_BIT;
        if (HasFlag(spec.usage, TextureUsage::ColorAttachment))
            vkUsage |= VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
        if (HasFlag(spec.usage, TextureUsage::DepthStencilAttachment))
            vkUsage |= VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT;
        if (HasFlag(spec.usage, TextureUsage::TransferSrc))
            vkUsage |= VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
        if (HasFlag(spec.usage, TextureUsage::TransferDst))
            vkUsage |= VK_IMAGE_USAGE_TRANSFER_DST_BIT;
        if (HasFlag(spec.usage, TextureUsage::InputAttachment))
            vkUsage |= VK_IMAGE_USAGE_INPUT_ATTACHMENT_BIT;
        imageInfo.usage = vkUsage;

        VmaAllocationCreateInfo allocInfo{};
        allocInfo.usage = VMA_MEMORY_USAGE_AUTO;
        allocInfo.flags = VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT;

        VkImage image = VK_NULL_HANDLE;
        VmaAllocation allocation = VK_NULL_HANDLE;

        if (vmaCreateImage(allocator, &imageInfo, &allocInfo, &image, &allocation, nullptr) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateTexture: vmaCreateImage failed");
            return NullTexture;
        }

        // Create image view
        VkImageViewCreateInfo viewInfo{};
        viewInfo.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
        viewInfo.image = image;
        viewInfo.viewType = spec.arrayLayers > 1 ? VK_IMAGE_VIEW_TYPE_2D_ARRAY : VK_IMAGE_VIEW_TYPE_2D;
        viewInfo.format = ToVulkan(spec.format);
        viewInfo.subresourceRange.aspectMask = IsDepthFormat(spec.format) ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
        viewInfo.subresourceRange.baseMipLevel = 0;
        viewInfo.subresourceRange.levelCount = spec.mipLevels;
        viewInfo.subresourceRange.baseArrayLayer = 0;
        viewInfo.subresourceRange.layerCount = spec.arrayLayers;

        VkImageView imageView = VK_NULL_HANDLE;
        if (vkCreateImageView(device, &viewInfo, nullptr, &imageView) != VK_SUCCESS)
        {
            vmaDestroyImage(allocator, image, allocation);
            GG_CORE_ERROR("RHIDevice::CreateTexture: vkCreateImageView failed");
            return NullTexture;
        }

        return VulkanResourceRegistry::Get().RegisterTexture(image, imageView, VK_NULL_HANDLE, allocation,
                                                              spec.width, spec.height, spec.format);
    }

    void RHIDevice::DestroyTexture(RHITextureHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto textureData = registry.GetTextureData(handle);
        auto device = VulkanContext::Get().GetDevice();
        auto allocator = VulkanContext::Get().GetAllocator();

        if (textureData.imageView != VK_NULL_HANDLE)
            vkDestroyImageView(device, textureData.imageView, nullptr);
        if (textureData.sampler != VK_NULL_HANDLE)
            vkDestroySampler(device, textureData.sampler, nullptr);
        if (textureData.image != VK_NULL_HANDLE)
            vmaDestroyImage(allocator, textureData.image, textureData.allocation);

        registry.UnregisterTexture(handle);
    }

    void RHIDevice::UploadTextureData(RHITextureHandle handle, const void* pixels, uint64_t size)
    {
        if (!handle.IsValid() || !pixels || size == 0) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto textureData = registry.GetTextureData(handle);

        // Create staging buffer
        RHIBufferSpecification stagingSpec;
        stagingSpec.size = size;
        stagingSpec.usage = BufferUsage::Staging;
        stagingSpec.cpuVisible = true;

        RHIBufferHandle staging = CreateBuffer(stagingSpec);
        UploadBufferData(staging, pixels, size, 0);

        VulkanContext::Get().ImmediateSubmit([&](VkCommandBuffer cmd) {
            // Transition to transfer dst
            VkImageMemoryBarrier barrier{};
            barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            barrier.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
            barrier.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.image = textureData.image;
            barrier.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            barrier.subresourceRange.baseMipLevel = 0;
            barrier.subresourceRange.levelCount = 1;
            barrier.subresourceRange.baseArrayLayer = 0;
            barrier.subresourceRange.layerCount = 1;
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;

            vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                 0, 0, nullptr, 0, nullptr, 1, &barrier);

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
            region.imageExtent = { textureData.width, textureData.height, 1 };

            vkCmdCopyBufferToImage(cmd, registry.GetBuffer(staging), textureData.image,
                                   VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);

            // Transition to shader read
            barrier.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
            barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;

            vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                                 0, 0, nullptr, 0, nullptr, 1, &barrier);
        });

        DestroyBuffer(staging);
    }

    uint32_t RHIDevice::GetTextureWidth(RHITextureHandle handle) const
    {
        if (!handle.IsValid()) return 0;
        return VulkanResourceRegistry::Get().GetTextureData(handle).width;
    }

    uint32_t RHIDevice::GetTextureHeight(RHITextureHandle handle) const
    {
        if (!handle.IsValid()) return 0;
        return VulkanResourceRegistry::Get().GetTextureData(handle).height;
    }

    // ============================================================================
    // Sampler Management
    // ============================================================================

    RHISamplerHandle RHIDevice::CreateSampler(const RHISamplerSpecification& spec)
    {
        auto device = VulkanContext::Get().GetDevice();

        VkSamplerCreateInfo samplerInfo{};
        samplerInfo.sType = VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO;
        samplerInfo.magFilter = ToVulkan(spec.magFilter);
        samplerInfo.minFilter = ToVulkan(spec.minFilter);
        samplerInfo.mipmapMode = ToVulkan(spec.mipmapMode);
        samplerInfo.addressModeU = ToVulkan(spec.addressModeU);
        samplerInfo.addressModeV = ToVulkan(spec.addressModeV);
        samplerInfo.addressModeW = ToVulkan(spec.addressModeW);
        samplerInfo.mipLodBias = spec.mipLodBias;
        samplerInfo.anisotropyEnable = spec.anisotropyEnable ? VK_TRUE : VK_FALSE;
        samplerInfo.maxAnisotropy = spec.maxAnisotropy;
        samplerInfo.compareEnable = spec.compareEnable ? VK_TRUE : VK_FALSE;
        samplerInfo.compareOp = ToVulkan(spec.compareOp);
        samplerInfo.minLod = spec.minLod;
        samplerInfo.maxLod = spec.maxLod;
        samplerInfo.borderColor = ToVulkan(spec.borderColor);
        samplerInfo.unnormalizedCoordinates = VK_FALSE;

        VkSampler sampler = VK_NULL_HANDLE;
        if (vkCreateSampler(device, &samplerInfo, nullptr, &sampler) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateSampler: vkCreateSampler failed");
            return NullSampler;
        }

        // Register in a simple way - we'll need to add sampler registration to the registry
        // For now, store sampler handle directly as ID (this works because VkSampler is a pointer/handle)
        return RHISamplerHandle{ reinterpret_cast<uint64_t>(sampler) };
    }

    void RHIDevice::DestroySampler(RHISamplerHandle handle)
    {
        if (!handle.IsValid()) return;
        auto device = VulkanContext::Get().GetDevice();
        VkSampler sampler = reinterpret_cast<VkSampler>(handle.id);
        vkDestroySampler(device, sampler, nullptr);
    }

    // ============================================================================
    // Shader Management
    // ============================================================================

    RHIShaderModuleHandle RHIDevice::CreateShaderModule(ShaderStage stage, const std::vector<char>& spirvCode)
    {
        return CreateShaderModule(stage, spirvCode.data(), spirvCode.size());
    }

    RHIShaderModuleHandle RHIDevice::CreateShaderModule(ShaderStage stage, const void* spirvData, size_t spirvSize)
    {
        auto device = VulkanContext::Get().GetDevice();

        VkShaderModuleCreateInfo createInfo{};
        createInfo.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO;
        createInfo.codeSize = spirvSize;
        createInfo.pCode = reinterpret_cast<const uint32_t*>(spirvData);

        VkShaderModule module = VK_NULL_HANDLE;
        if (vkCreateShaderModule(device, &createInfo, nullptr, &module) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateShaderModule: vkCreateShaderModule failed");
            return NullShaderModule;
        }

        return VulkanResourceRegistry::Get().RegisterShaderModule(module, stage);
    }

    void RHIDevice::DestroyShaderModule(RHIShaderModuleHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto moduleData = registry.GetShaderModuleData(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (moduleData.module != VK_NULL_HANDLE)
            vkDestroyShaderModule(device, moduleData.module, nullptr);

        registry.UnregisterShaderModule(handle);
    }

    // ============================================================================
    // Pipeline Management
    // ============================================================================

    RHIGraphicsPipelineResult RHIDevice::CreateGraphicsPipeline(const RHIGraphicsPipelineSpecification& spec)
    {
        RHIGraphicsPipelineResult result;
        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        // Shader stages
        // Store module data to keep entry point strings alive until after vkCreateGraphicsPipelines
        std::vector<VulkanResourceRegistry::ShaderModuleData> moduleDataStorage;
        moduleDataStorage.reserve(spec.shaderModules.size());
        for (const auto& moduleHandle : spec.shaderModules)
        {
            auto moduleData = registry.GetShaderModuleData(moduleHandle);
            if (moduleData.module != VK_NULL_HANDLE)
                moduleDataStorage.push_back(moduleData);
        }

        std::vector<VkPipelineShaderStageCreateInfo> shaderStages;
        shaderStages.reserve(moduleDataStorage.size());
        for (const auto& moduleData : moduleDataStorage)
        {
            VkPipelineShaderStageCreateInfo stageInfo{};
            stageInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
            stageInfo.stage = static_cast<VkShaderStageFlagBits>(ToVulkan(moduleData.stage));
            stageInfo.module = moduleData.module;
            stageInfo.pName = moduleData.entryPoint.c_str();
            shaderStages.push_back(stageInfo);
        }

        // Vertex input
        std::vector<VkVertexInputBindingDescription> vkBindings;
        std::vector<VkVertexInputAttributeDescription> vkAttributes;
        for (const auto& binding : spec.vertexBindings)
            vkBindings.push_back(ToVulkan(binding));
        for (const auto& attr : spec.vertexAttributes)
            vkAttributes.push_back(ToVulkan(attr));

        VkPipelineVertexInputStateCreateInfo vertexInputInfo{};
        vertexInputInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO;
        vertexInputInfo.vertexBindingDescriptionCount = static_cast<uint32_t>(vkBindings.size());
        vertexInputInfo.pVertexBindingDescriptions = vkBindings.data();
        vertexInputInfo.vertexAttributeDescriptionCount = static_cast<uint32_t>(vkAttributes.size());
        vertexInputInfo.pVertexAttributeDescriptions = vkAttributes.data();

        // Input assembly
        VkPipelineInputAssemblyStateCreateInfo inputAssembly{};
        inputAssembly.sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO;
        inputAssembly.topology = ToVulkan(spec.topology);
        inputAssembly.primitiveRestartEnable = VK_FALSE;

        // Viewport state (dynamic)
        VkPipelineViewportStateCreateInfo viewportState{};
        viewportState.sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO;
        viewportState.viewportCount = 1;
        viewportState.scissorCount = 1;

        // Rasterization
        VkPipelineRasterizationStateCreateInfo rasterizer{};
        rasterizer.sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO;
        rasterizer.depthClampEnable = VK_FALSE;
        rasterizer.rasterizerDiscardEnable = VK_FALSE;
        rasterizer.polygonMode = ToVulkan(spec.polygonMode);
        rasterizer.cullMode = ToVulkan(spec.cullMode);
        rasterizer.frontFace = ToVulkan(spec.frontFace);
        rasterizer.lineWidth = spec.lineWidth;

        // Multisampling
        VkPipelineMultisampleStateCreateInfo multisampling{};
        multisampling.sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO;
        multisampling.sampleShadingEnable = VK_FALSE;
        multisampling.rasterizationSamples = ToVulkan(spec.samples);

        // Depth stencil
        VkPipelineDepthStencilStateCreateInfo depthStencil{};
        depthStencil.sType = VK_STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO;
        depthStencil.depthTestEnable = spec.depthTestEnable ? VK_TRUE : VK_FALSE;
        depthStencil.depthWriteEnable = spec.depthWriteEnable ? VK_TRUE : VK_FALSE;
        depthStencil.depthCompareOp = ToVulkan(spec.depthCompareOp);

        // Color blending
        std::vector<VkPipelineColorBlendAttachmentState> colorBlendAttachments;
        for (const auto& blend : spec.colorBlendStates)
        {
            VkPipelineColorBlendAttachmentState attachment{};
            attachment.blendEnable = blend.enable ? VK_TRUE : VK_FALSE;
            attachment.srcColorBlendFactor = ToVulkan(blend.srcColorFactor);
            attachment.dstColorBlendFactor = ToVulkan(blend.dstColorFactor);
            attachment.colorBlendOp = ToVulkan(blend.colorOp);
            attachment.srcAlphaBlendFactor = ToVulkan(blend.srcAlphaFactor);
            attachment.dstAlphaBlendFactor = ToVulkan(blend.dstAlphaFactor);
            attachment.alphaBlendOp = ToVulkan(blend.alphaOp);
            attachment.colorWriteMask = ToVulkan(blend.colorWriteMask);
            colorBlendAttachments.push_back(attachment);
        }
        if (colorBlendAttachments.empty())
        {
            VkPipelineColorBlendAttachmentState attachment{};
            attachment.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                                        VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
            colorBlendAttachments.push_back(attachment);
        }

        VkPipelineColorBlendStateCreateInfo colorBlending{};
        colorBlending.sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO;
        colorBlending.logicOpEnable = VK_FALSE;
        colorBlending.attachmentCount = static_cast<uint32_t>(colorBlendAttachments.size());
        colorBlending.pAttachments = colorBlendAttachments.data();

        // Dynamic state
        std::vector<VkDynamicState> dynamicStates = { VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR };
        VkPipelineDynamicStateCreateInfo dynamicState{};
        dynamicState.sType = VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO;
        dynamicState.dynamicStateCount = static_cast<uint32_t>(dynamicStates.size());
        dynamicState.pDynamicStates = dynamicStates.data();

        // Pipeline layout
        std::vector<VkDescriptorSetLayout> vkSetLayouts;
        for (const auto& layoutHandle : spec.descriptorSetLayouts)
            vkSetLayouts.push_back(registry.GetDescriptorSetLayout(layoutHandle));

        std::vector<VkPushConstantRange> vkPushConstants;
        for (const auto& pc : spec.pushConstantRanges)
        {
            VkPushConstantRange range{};
            range.stageFlags = ToVulkan(pc.stages);
            range.offset = pc.offset;
            range.size = pc.size;
            vkPushConstants.push_back(range);
        }

        VkPipelineLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO;
        layoutInfo.setLayoutCount = static_cast<uint32_t>(vkSetLayouts.size());
        layoutInfo.pSetLayouts = vkSetLayouts.data();
        layoutInfo.pushConstantRangeCount = static_cast<uint32_t>(vkPushConstants.size());
        layoutInfo.pPushConstantRanges = vkPushConstants.data();

        VkPipelineLayout pipelineLayout = VK_NULL_HANDLE;
        if (vkCreatePipelineLayout(device, &layoutInfo, nullptr, &pipelineLayout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateGraphicsPipeline: vkCreatePipelineLayout failed");
            return result;
        }

        // Create pipeline
        VkGraphicsPipelineCreateInfo pipelineInfo{};
        pipelineInfo.sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO;
        pipelineInfo.stageCount = static_cast<uint32_t>(shaderStages.size());
        pipelineInfo.pStages = shaderStages.data();
        pipelineInfo.pVertexInputState = &vertexInputInfo;
        pipelineInfo.pInputAssemblyState = &inputAssembly;
        pipelineInfo.pViewportState = &viewportState;
        pipelineInfo.pRasterizationState = &rasterizer;
        pipelineInfo.pMultisampleState = &multisampling;
        pipelineInfo.pDepthStencilState = &depthStencil;
        pipelineInfo.pColorBlendState = &colorBlending;
        pipelineInfo.pDynamicState = &dynamicState;
        pipelineInfo.layout = pipelineLayout;
        pipelineInfo.renderPass = registry.GetRenderPass(spec.renderPass);
        pipelineInfo.subpass = spec.subpass;

        VkPipeline pipeline = VK_NULL_HANDLE;
        if (vkCreateGraphicsPipelines(device, VK_NULL_HANDLE, 1, &pipelineInfo, nullptr, &pipeline) != VK_SUCCESS)
        {
            vkDestroyPipelineLayout(device, pipelineLayout, nullptr);
            GG_CORE_ERROR("RHIDevice::CreateGraphicsPipeline: vkCreateGraphicsPipelines failed");
            return result;
        }

        result.pipeline = registry.RegisterPipeline(pipeline, pipelineLayout);
        result.layout = registry.RegisterPipelineLayout(pipelineLayout);
        return result;
    }

    void RHIDevice::DestroyPipeline(RHIPipelineHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto pipelineData = registry.GetPipelineData(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (pipelineData.pipeline != VK_NULL_HANDLE)
            vkDestroyPipeline(device, pipelineData.pipeline, nullptr);
        // Note: layout is destroyed separately via DestroyPipelineLayout

        registry.UnregisterPipeline(handle);
    }

    void RHIDevice::DestroyPipelineLayout(RHIPipelineLayoutHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        VkPipelineLayout layout = registry.GetPipelineLayout(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (layout != VK_NULL_HANDLE)
            vkDestroyPipelineLayout(device, layout, nullptr);

        registry.UnregisterPipelineLayout(handle);
    }

    RHIPipelineLayoutHandle RHIDevice::GetPipelineLayout(RHIPipelineHandle pipeline) const
    {
        if (!pipeline.IsValid()) return NullPipelineLayout;

        auto& registry = VulkanResourceRegistry::Get();
        auto pipelineData = registry.GetPipelineData(pipeline);

        // Find the layout handle for this pipeline's layout
        // Since RegisterPipeline stores the layout, we need to look it up
        // For now, return a handle with the layout pointer as ID
        return RHIPipelineLayoutHandle{ reinterpret_cast<uint64_t>(pipelineData.layout) };
    }

    // ============================================================================
    // Render Pass Management
    // ============================================================================

    RHIRenderPassHandle RHIDevice::CreateRenderPass(const RHIRenderPassSpecification& spec)
    {
        auto device = VulkanContext::Get().GetDevice();

        std::vector<VkAttachmentDescription> attachments;
        std::vector<VkAttachmentReference> colorRefs;

        for (const auto& colorAttach : spec.colorAttachments)
        {
            VkAttachmentDescription attachment{};
            attachment.format = ToVulkan(colorAttach.format);
            attachment.samples = ToVulkan(colorAttach.samples);
            attachment.loadOp = ToVulkan(colorAttach.loadOp);
            attachment.storeOp = ToVulkan(colorAttach.storeOp);
            attachment.stencilLoadOp = ToVulkan(colorAttach.stencilLoadOp);
            attachment.stencilStoreOp = ToVulkan(colorAttach.stencilStoreOp);
            attachment.initialLayout = ToVulkan(colorAttach.initialLayout);
            attachment.finalLayout = ToVulkan(colorAttach.finalLayout);
            attachments.push_back(attachment);

            VkAttachmentReference ref{};
            ref.attachment = static_cast<uint32_t>(colorRefs.size());
            ref.layout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
            colorRefs.push_back(ref);
        }

        VkAttachmentReference depthRef{};
        bool hasDepth = spec.depthStencilAttachment.has_value();
        if (hasDepth)
        {
            const auto& depthAttach = spec.depthStencilAttachment.value();
            VkAttachmentDescription attachment{};
            attachment.format = ToVulkan(depthAttach.format);
            attachment.samples = ToVulkan(depthAttach.samples);
            attachment.loadOp = ToVulkan(depthAttach.loadOp);
            attachment.storeOp = ToVulkan(depthAttach.storeOp);
            attachment.stencilLoadOp = ToVulkan(depthAttach.stencilLoadOp);
            attachment.stencilStoreOp = ToVulkan(depthAttach.stencilStoreOp);
            attachment.initialLayout = ToVulkan(depthAttach.initialLayout);
            attachment.finalLayout = ToVulkan(depthAttach.finalLayout);
            attachments.push_back(attachment);

            depthRef.attachment = static_cast<uint32_t>(attachments.size() - 1);
            depthRef.layout = VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
        }

        VkSubpassDescription subpass{};
        subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
        subpass.colorAttachmentCount = static_cast<uint32_t>(colorRefs.size());
        subpass.pColorAttachments = colorRefs.data();
        subpass.pDepthStencilAttachment = hasDepth ? &depthRef : nullptr;

        VkSubpassDependency dependency{};
        dependency.srcSubpass = VK_SUBPASS_EXTERNAL;
        dependency.dstSubpass = 0;
        dependency.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.srcAccessMask = 0;
        dependency.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;

        VkRenderPassCreateInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO;
        renderPassInfo.attachmentCount = static_cast<uint32_t>(attachments.size());
        renderPassInfo.pAttachments = attachments.data();
        renderPassInfo.subpassCount = 1;
        renderPassInfo.pSubpasses = &subpass;
        renderPassInfo.dependencyCount = 1;
        renderPassInfo.pDependencies = &dependency;

        VkRenderPass renderPass = VK_NULL_HANDLE;
        if (vkCreateRenderPass(device, &renderPassInfo, nullptr, &renderPass) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateRenderPass: vkCreateRenderPass failed");
            return NullRenderPass;
        }

        return VulkanResourceRegistry::Get().RegisterRenderPass(renderPass);
    }

    void RHIDevice::DestroyRenderPass(RHIRenderPassHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto rpData = registry.GetRenderPassData(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (rpData.renderPass != VK_NULL_HANDLE)
            vkDestroyRenderPass(device, rpData.renderPass, nullptr);

        registry.UnregisterRenderPass(handle);
    }

    // ============================================================================
    // Framebuffer Management
    // ============================================================================

    RHIFramebufferHandle RHIDevice::CreateFramebuffer(const RHIFramebufferSpecification& spec)
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        std::vector<VkImageView> attachmentViews;
        for (const auto& texHandle : spec.attachments)
        {
            auto texData = registry.GetTextureData(texHandle);
            if (texData.imageView == VK_NULL_HANDLE)
            {
                GG_CORE_ERROR("RHIDevice::CreateFramebuffer: Invalid texture attachment (handle.id={})", texHandle.id);
                return NullFramebuffer;
            }
            attachmentViews.push_back(texData.imageView);
        }

        VkRenderPass vkRenderPass = registry.GetRenderPass(spec.renderPass);
        if (vkRenderPass == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("RHIDevice::CreateFramebuffer: Invalid render pass handle (id={})", spec.renderPass.id);
            return NullFramebuffer;
        }

        VkFramebufferCreateInfo framebufferInfo{};
        framebufferInfo.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO;
        framebufferInfo.renderPass = vkRenderPass;
        framebufferInfo.attachmentCount = static_cast<uint32_t>(attachmentViews.size());
        framebufferInfo.pAttachments = attachmentViews.data();
        framebufferInfo.width = spec.width;
        framebufferInfo.height = spec.height;
        framebufferInfo.layers = spec.layers;

        VkFramebuffer framebuffer = VK_NULL_HANDLE;
        if (vkCreateFramebuffer(device, &framebufferInfo, nullptr, &framebuffer) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateFramebuffer: vkCreateFramebuffer failed");
            return NullFramebuffer;
        }

        // Register framebuffer - we'll need to add this to registry
        // For now, return handle with VkFramebuffer as ID
        return RHIFramebufferHandle{ reinterpret_cast<uint64_t>(framebuffer) };
    }

    void RHIDevice::DestroyFramebuffer(RHIFramebufferHandle handle)
    {
        if (!handle.IsValid()) return;

        auto device = VulkanContext::Get().GetDevice();
        VkFramebuffer framebuffer = reinterpret_cast<VkFramebuffer>(handle.id);
        vkDestroyFramebuffer(device, framebuffer, nullptr);
    }

    // ============================================================================
    // Bindless Texture Support
    // ============================================================================

    RHIDescriptorSetLayoutHandle RHIDevice::CreateBindlessTextureLayout(uint32_t maxTextures)
    {
        auto device = VulkanContext::Get().GetDevice();

        VkDescriptorSetLayoutBinding binding{};
        binding.binding = 0;
        binding.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        binding.descriptorCount = maxTextures;
        binding.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;

        VkDescriptorBindingFlags bindingFlags =
            VK_DESCRIPTOR_BINDING_PARTIALLY_BOUND_BIT |
            VK_DESCRIPTOR_BINDING_UPDATE_AFTER_BIND_BIT |
            VK_DESCRIPTOR_BINDING_VARIABLE_DESCRIPTOR_COUNT_BIT;

        VkDescriptorSetLayoutBindingFlagsCreateInfo flagsInfo{};
        flagsInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_BINDING_FLAGS_CREATE_INFO;
        flagsInfo.bindingCount = 1;
        flagsInfo.pBindingFlags = &bindingFlags;

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.pNext = &flagsInfo;
        layoutInfo.flags = VK_DESCRIPTOR_SET_LAYOUT_CREATE_UPDATE_AFTER_BIND_POOL_BIT;
        layoutInfo.bindingCount = 1;
        layoutInfo.pBindings = &binding;

        VkDescriptorSetLayout layout = VK_NULL_HANDLE;
        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateBindlessTextureLayout: vkCreateDescriptorSetLayout failed");
            return NullDescriptorSetLayout;
        }

        return VulkanResourceRegistry::Get().RegisterDescriptorSetLayout(layout);
    }

    RHIDescriptorSetHandle RHIDevice::AllocateBindlessDescriptorSet(RHIDescriptorSetLayoutHandle layoutHandle, uint32_t maxTextures)
    {
        if (!layoutHandle.IsValid()) return NullDescriptorSet;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(layoutHandle);

        // Create a dedicated pool for bindless
        VkDescriptorPoolSize poolSize{};
        poolSize.type = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        poolSize.descriptorCount = maxTextures;

        VkDescriptorPoolCreateInfo poolInfo{};
        poolInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO;
        poolInfo.flags = VK_DESCRIPTOR_POOL_CREATE_UPDATE_AFTER_BIND_BIT;
        poolInfo.maxSets = 1;
        poolInfo.poolSizeCount = 1;
        poolInfo.pPoolSizes = &poolSize;

        VkDescriptorPool pool = VK_NULL_HANDLE;
        if (vkCreateDescriptorPool(device, &poolInfo, nullptr, &pool) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::AllocateBindlessDescriptorSet: vkCreateDescriptorPool failed");
            return NullDescriptorSet;
        }

        uint32_t variableCount = maxTextures;
        VkDescriptorSetVariableDescriptorCountAllocateInfo variableInfo{};
        variableInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_VARIABLE_DESCRIPTOR_COUNT_ALLOCATE_INFO;
        variableInfo.descriptorSetCount = 1;
        variableInfo.pDescriptorCounts = &variableCount;

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.pNext = &variableInfo;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &layout;

        VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
        if (vkAllocateDescriptorSets(device, &allocInfo, &descriptorSet) != VK_SUCCESS)
        {
            vkDestroyDescriptorPool(device, pool, nullptr);
            GG_CORE_ERROR("RHIDevice::AllocateBindlessDescriptorSet: vkAllocateDescriptorSets failed");
            return NullDescriptorSet;
        }

        // Register with the owning pool so it gets destroyed when the set is freed
        return registry.RegisterDescriptorSet(descriptorSet, layoutHandle, pool);
    }

    void RHIDevice::UpdateBindlessTexture(RHIDescriptorSetHandle setHandle, uint32_t index,
                                           RHITextureHandle textureHandle, RHISamplerHandle samplerHandle)
    {
        if (!setHandle.IsValid() || !textureHandle.IsValid()) return;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        VkDescriptorSet vkSet = registry.GetDescriptorSet(setHandle);
        auto textureData = registry.GetTextureData(textureHandle);
        VkSampler sampler = reinterpret_cast<VkSampler>(samplerHandle.id);

        VkDescriptorImageInfo imageInfo{};
        imageInfo.sampler = sampler;
        imageInfo.imageView = textureData.imageView;
        imageInfo.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = vkSet;
        write.dstBinding = 0;
        write.dstArrayElement = index;
        write.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        write.descriptorCount = 1;
        write.pImageInfo = &imageInfo;

        vkUpdateDescriptorSets(device, 1, &write, 0, nullptr);
    }

    uint32_t RHIDevice::GetMaxBindlessTextures() const
    {
        const auto& limits = VulkanContext::Get().GetBindlessLimits();
        return limits.maxPerStageDescriptorSampledImages;
    }

    void* RHIDevice::GetRawDescriptorSet(RHIDescriptorSetHandle handle) const
    {
        if (!handle.IsValid()) return nullptr;
        return VulkanResourceRegistry::Get().GetDescriptorSet(handle);
    }

    // ============================================================================
    // Bindless Texture Support (Separate Sampler Pattern)
    // ============================================================================

    RHIDescriptorSetLayoutHandle RHIDevice::CreateBindlessSamplerTextureLayout(
        RHISamplerHandle immutableSampler, uint32_t maxTextures)
    {
        auto device = VulkanContext::Get().GetDevice();
        VkSampler sampler = reinterpret_cast<VkSampler>(immutableSampler.id);

        // Two bindings: binding 0 = immutable sampler, binding 1 = texture array
        VkDescriptorSetLayoutBinding bindings[2] = {};

        // Binding 0: Immutable sampler
        bindings[0].binding = 0;
        bindings[0].descriptorType = VK_DESCRIPTOR_TYPE_SAMPLER;
        bindings[0].descriptorCount = 1;
        bindings[0].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
        bindings[0].pImmutableSamplers = &sampler;

        // Binding 1: Texture array (SAMPLED_IMAGE)
        bindings[1].binding = 1;
        bindings[1].descriptorType = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE;
        bindings[1].descriptorCount = maxTextures;
        bindings[1].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
        bindings[1].pImmutableSamplers = nullptr;

        // Binding flags: sampler has none, texture array needs bindless flags
        VkDescriptorBindingFlags bindingFlags[2] = {};
        bindingFlags[0] = 0;
        bindingFlags[1] = VK_DESCRIPTOR_BINDING_PARTIALLY_BOUND_BIT |
                         VK_DESCRIPTOR_BINDING_UPDATE_AFTER_BIND_BIT |
                         VK_DESCRIPTOR_BINDING_VARIABLE_DESCRIPTOR_COUNT_BIT;

        VkDescriptorSetLayoutBindingFlagsCreateInfo flagsInfo{};
        flagsInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_BINDING_FLAGS_CREATE_INFO;
        flagsInfo.bindingCount = 2;
        flagsInfo.pBindingFlags = bindingFlags;

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.pNext = &flagsInfo;
        layoutInfo.flags = VK_DESCRIPTOR_SET_LAYOUT_CREATE_UPDATE_AFTER_BIND_POOL_BIT;
        layoutInfo.bindingCount = 2;
        layoutInfo.pBindings = bindings;

        VkDescriptorSetLayout layout = VK_NULL_HANDLE;
        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateBindlessSamplerTextureLayout: vkCreateDescriptorSetLayout failed");
            return NullDescriptorSetLayout;
        }

        return VulkanResourceRegistry::Get().RegisterDescriptorSetLayout(layout);
    }

    RHIDescriptorSetHandle RHIDevice::AllocateBindlessSamplerTextureSet(
        RHIDescriptorSetLayoutHandle layoutHandle, uint32_t maxTextures)
    {
        if (!layoutHandle.IsValid()) return NullDescriptorSet;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(layoutHandle);

        // Create pool with UPDATE_AFTER_BIND for both sampler and sampled images
        VkDescriptorPoolSize poolSizes[2] = {};
        poolSizes[0].type = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE;
        poolSizes[0].descriptorCount = maxTextures;
        poolSizes[1].type = VK_DESCRIPTOR_TYPE_SAMPLER;
        poolSizes[1].descriptorCount = 1;

        VkDescriptorPoolCreateInfo poolInfo{};
        poolInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO;
        poolInfo.flags = VK_DESCRIPTOR_POOL_CREATE_UPDATE_AFTER_BIND_BIT;
        poolInfo.maxSets = 1;
        poolInfo.poolSizeCount = 2;
        poolInfo.pPoolSizes = poolSizes;

        VkDescriptorPool pool = VK_NULL_HANDLE;
        if (vkCreateDescriptorPool(device, &poolInfo, nullptr, &pool) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::AllocateBindlessSamplerTextureSet: vkCreateDescriptorPool failed");
            return NullDescriptorSet;
        }

        // Allocate with variable descriptor count (for texture array at binding 1)
        VkDescriptorSetVariableDescriptorCountAllocateInfo variableInfo{};
        variableInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_VARIABLE_DESCRIPTOR_COUNT_ALLOCATE_INFO;
        variableInfo.descriptorSetCount = 1;
        variableInfo.pDescriptorCounts = &maxTextures;

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.pNext = &variableInfo;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &layout;

        VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
        if (vkAllocateDescriptorSets(device, &allocInfo, &descriptorSet) != VK_SUCCESS)
        {
            vkDestroyDescriptorPool(device, pool, nullptr);
            GG_CORE_ERROR("RHIDevice::AllocateBindlessSamplerTextureSet: vkAllocateDescriptorSets failed");
            return NullDescriptorSet;
        }

        // Register with the owning pool so it gets destroyed when the set is freed
        return registry.RegisterDescriptorSet(descriptorSet, layoutHandle, pool);
    }

    void RHIDevice::UpdateBindlessSamplerTextureSlot(
        RHIDescriptorSetHandle setHandle, uint32_t index, RHITextureHandle textureHandle)
    {
        if (!setHandle.IsValid() || !textureHandle.IsValid()) return;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        VkDescriptorSet vkSet = registry.GetDescriptorSet(setHandle);
        auto textureData = registry.GetTextureData(textureHandle);

        VkDescriptorImageInfo imageInfo{};
        imageInfo.sampler = VK_NULL_HANDLE;  // Not used for SAMPLED_IMAGE
        imageInfo.imageView = textureData.imageView;
        imageInfo.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;

        VkWriteDescriptorSet write{};
        write.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        write.dstSet = vkSet;
        write.dstBinding = 1;  // Texture array is at binding 1
        write.dstArrayElement = index;
        write.descriptorType = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE;
        write.descriptorCount = 1;
        write.pImageInfo = &imageInfo;

        vkUpdateDescriptorSets(device, 1, &write, 0, nullptr);
    }

    // ============================================================================
    // ImGui Integration
    // ============================================================================

    void* RHIDevice::RegisterImGuiTexture(RHITextureHandle texture, RHISamplerHandle sampler)
    {
        if (!texture.IsValid() || !sampler.IsValid()) return nullptr;

        auto& registry = VulkanResourceRegistry::Get();
        auto textureData = registry.GetTextureData(texture);
        VkSampler vkSampler = reinterpret_cast<VkSampler>(sampler.id);

        return ImGui_ImplVulkan_AddTexture(
            vkSampler, textureData.imageView, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);
    }

    void RHIDevice::UnregisterImGuiTexture(void* imguiHandle)
    {
        if (imguiHandle)
        {
            ImGui_ImplVulkan_RemoveTexture(static_cast<VkDescriptorSet>(imguiHandle));
        }
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

    void RHICmd::PushConstants(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layoutHandle,
                               ShaderStage stages, uint32_t offset, uint32_t size, const void* data)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout layout = registry.GetPipelineLayout(layoutHandle);
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

        if (vkCmd == VK_NULL_HANDLE || rpData.renderPass == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("BeginRenderPass: Invalid command buffer or render pass");
            return;
        }

        // Use explicit framebuffer handle if provided, otherwise fall back to render pass data
        VkFramebuffer vkFramebuffer = framebuffer.IsValid()
            ? reinterpret_cast<VkFramebuffer>(framebuffer.id)
            : rpData.framebuffer;

        if (vkFramebuffer == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("BeginRenderPass: Invalid framebuffer");
            return;
        }

        VkRenderPassBeginInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO;
        renderPassInfo.renderPass = rpData.renderPass;
        renderPassInfo.framebuffer = vkFramebuffer;
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

    // ============================================================================
    // RHICmd: Transfer Commands
    // ============================================================================

    void RHICmd::CopyBuffer(RHICommandBufferHandle cmd, RHIBufferHandle src, RHIBufferHandle dst,
                            uint64_t srcOffset, uint64_t dstOffset, uint64_t size)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer srcBuffer = registry.GetBuffer(src);
        VkBuffer dstBuffer = registry.GetBuffer(dst);
        if (vkCmd == VK_NULL_HANDLE || srcBuffer == VK_NULL_HANDLE || dstBuffer == VK_NULL_HANDLE) return;

        VkBufferCopy copyRegion{};
        copyRegion.srcOffset = srcOffset;
        copyRegion.dstOffset = dstOffset;
        copyRegion.size = size;

        vkCmdCopyBuffer(vkCmd, srcBuffer, dstBuffer, 1, &copyRegion);
    }

    void RHICmd::CopyBufferToTexture(RHICommandBufferHandle cmd, RHIBufferHandle buffer,
                                     RHITextureHandle texture, const RHIBufferImageCopy& region)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkBuffer vkBuffer = registry.GetBuffer(buffer);
        auto texData = registry.GetTextureData(texture);
        if (vkCmd == VK_NULL_HANDLE || vkBuffer == VK_NULL_HANDLE || texData.image == VK_NULL_HANDLE) return;

        VkBufferImageCopy vkRegion{};
        vkRegion.bufferOffset = region.bufferOffset;
        vkRegion.bufferRowLength = region.bufferRowLength;
        vkRegion.bufferImageHeight = region.bufferImageHeight;
        vkRegion.imageSubresource.aspectMask = IsDepthFormat(texData.format) ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
        vkRegion.imageSubresource.mipLevel = region.mipLevel;
        vkRegion.imageSubresource.baseArrayLayer = region.arrayLayer;
        vkRegion.imageSubresource.layerCount = region.layerCount;
        vkRegion.imageOffset = { static_cast<int32_t>(region.imageOffsetX),
                                 static_cast<int32_t>(region.imageOffsetY),
                                 static_cast<int32_t>(region.imageOffsetZ) };
        vkRegion.imageExtent = { region.imageWidth, region.imageHeight, region.imageDepth };

        vkCmdCopyBufferToImage(vkCmd, vkBuffer, texData.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &vkRegion);
    }

    void RHICmd::CopyBufferToTexture(RHICommandBufferHandle cmd, RHIBufferHandle buffer,
                                     RHITextureHandle texture, uint32_t width, uint32_t height)
    {
        RHIBufferImageCopy region{};
        region.bufferOffset = 0;
        region.bufferRowLength = 0;
        region.bufferImageHeight = 0;
        region.imageOffsetX = 0;
        region.imageOffsetY = 0;
        region.imageOffsetZ = 0;
        region.imageWidth = width;
        region.imageHeight = height;
        region.imageDepth = 1;
        region.mipLevel = 0;
        region.arrayLayer = 0;
        region.layerCount = 1;

        CopyBufferToTexture(cmd, buffer, texture, region);
    }

    // ============================================================================
    // RHICmd: Image Layout Transitions
    // ============================================================================

    void RHICmd::TransitionImageLayout(RHICommandBufferHandle cmd, RHITextureHandle texture,
                                       ImageLayout oldLayout, ImageLayout newLayout)
    {
        auto& registry = VulkanResourceRegistry::Get();
        auto texData = registry.GetTextureData(texture);

        TransitionImageLayout(cmd, texture, oldLayout, newLayout, 0, 1, 0, 1);
    }

    void RHICmd::TransitionImageLayout(RHICommandBufferHandle cmd, RHITextureHandle texture,
                                       ImageLayout oldLayout, ImageLayout newLayout,
                                       uint32_t baseMipLevel, uint32_t mipCount,
                                       uint32_t baseArrayLayer, uint32_t layerCount)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        auto texData = registry.GetTextureData(texture);
        if (vkCmd == VK_NULL_HANDLE || texData.image == VK_NULL_HANDLE) return;

        VkImageMemoryBarrier barrier{};
        barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
        barrier.oldLayout = ToVulkan(oldLayout);
        barrier.newLayout = ToVulkan(newLayout);
        barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        barrier.image = texData.image;
        barrier.subresourceRange.aspectMask = IsDepthFormat(texData.format) ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
        barrier.subresourceRange.baseMipLevel = baseMipLevel;
        barrier.subresourceRange.levelCount = mipCount;
        barrier.subresourceRange.baseArrayLayer = baseArrayLayer;
        barrier.subresourceRange.layerCount = layerCount;

        VkPipelineStageFlags srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
        VkPipelineStageFlags dstStage = VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT;

        // Determine access masks and pipeline stages based on layouts
        if (oldLayout == ImageLayout::Undefined && newLayout == ImageLayout::TransferDst)
        {
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
            dstStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
        }
        else if (oldLayout == ImageLayout::TransferDst && newLayout == ImageLayout::ShaderReadOnly)
        {
            barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
            srcStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
            dstStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
        }
        else if (oldLayout == ImageLayout::Undefined && newLayout == ImageLayout::ColorAttachment)
        {
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
            dstStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        }
        else if (oldLayout == ImageLayout::ColorAttachment && newLayout == ImageLayout::ShaderReadOnly)
        {
            barrier.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
            srcStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            dstStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
        }
        else if (oldLayout == ImageLayout::ShaderReadOnly && newLayout == ImageLayout::ColorAttachment)
        {
            barrier.srcAccessMask = VK_ACCESS_SHADER_READ_BIT;
            barrier.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            dstStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        }
        else if (oldLayout == ImageLayout::Undefined && newLayout == ImageLayout::DepthStencilAttachment)
        {
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
            dstStage = VK_PIPELINE_STAGE_EARLY_FRAGMENT_TESTS_BIT;
        }
        else if (oldLayout == ImageLayout::ShaderReadOnly && newLayout == ImageLayout::TransferDst)
        {
            barrier.srcAccessMask = VK_ACCESS_SHADER_READ_BIT;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            dstStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
        }
        else if (oldLayout == ImageLayout::ColorAttachment && newLayout == ImageLayout::Present)
        {
            barrier.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
            barrier.dstAccessMask = 0;
            srcStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            dstStage = VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT;
        }
        else
        {
            // Generic fallback - all stages
            barrier.srcAccessMask = VK_ACCESS_MEMORY_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_MEMORY_READ_BIT | VK_ACCESS_MEMORY_WRITE_BIT;
            srcStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
            dstStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
        }

        vkCmdPipelineBarrier(vkCmd, srcStage, dstStage, 0,
                             0, nullptr, 0, nullptr, 1, &barrier);
    }

    void RHICmd::PipelineBarrier(RHICommandBufferHandle cmd, const RHIPipelineBarrier& barrier)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        if (vkCmd == VK_NULL_HANDLE) return;

        std::vector<VkImageMemoryBarrier> imageBarriers;
        imageBarriers.reserve(barrier.imageBarriers.size());

        VkPipelineStageFlags srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
        VkPipelineStageFlags dstStage = VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT;

        for (const auto& imgBarrier : barrier.imageBarriers)
        {
            auto texData = registry.GetTextureData(imgBarrier.texture);
            if (texData.image == VK_NULL_HANDLE) continue;

            VkImageMemoryBarrier vkBarrier{};
            vkBarrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            vkBarrier.oldLayout = ToVulkan(imgBarrier.oldLayout);
            vkBarrier.newLayout = ToVulkan(imgBarrier.newLayout);
            vkBarrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            vkBarrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            vkBarrier.image = texData.image;
            vkBarrier.subresourceRange.aspectMask = IsDepthFormat(texData.format) ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
            vkBarrier.subresourceRange.baseMipLevel = imgBarrier.baseMipLevel;
            vkBarrier.subresourceRange.levelCount = imgBarrier.mipCount;
            vkBarrier.subresourceRange.baseArrayLayer = imgBarrier.baseArrayLayer;
            vkBarrier.subresourceRange.layerCount = imgBarrier.layerCount;

            // Simplified access mask determination
            if (imgBarrier.oldLayout == ImageLayout::Undefined)
            {
                vkBarrier.srcAccessMask = 0;
                srcStage = VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT;
            }
            else if (imgBarrier.oldLayout == ImageLayout::TransferDst)
            {
                vkBarrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
                srcStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
            }
            else if (imgBarrier.oldLayout == ImageLayout::ColorAttachment)
            {
                vkBarrier.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
                srcStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            }
            else
            {
                vkBarrier.srcAccessMask = VK_ACCESS_MEMORY_READ_BIT | VK_ACCESS_MEMORY_WRITE_BIT;
                srcStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
            }

            if (imgBarrier.newLayout == ImageLayout::ShaderReadOnly)
            {
                vkBarrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
                dstStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
            }
            else if (imgBarrier.newLayout == ImageLayout::TransferDst)
            {
                vkBarrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
                dstStage = VK_PIPELINE_STAGE_TRANSFER_BIT;
            }
            else if (imgBarrier.newLayout == ImageLayout::ColorAttachment)
            {
                vkBarrier.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
                dstStage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
            }
            else
            {
                vkBarrier.dstAccessMask = VK_ACCESS_MEMORY_READ_BIT | VK_ACCESS_MEMORY_WRITE_BIT;
                dstStage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
            }

            imageBarriers.push_back(vkBarrier);
        }

        if (!imageBarriers.empty())
        {
            vkCmdPipelineBarrier(vkCmd, srcStage, dstStage, 0,
                                 0, nullptr, 0, nullptr,
                                 static_cast<uint32_t>(imageBarriers.size()), imageBarriers.data());
        }
    }

    // ============================================================================
    // RHICmd: Descriptor Set Binding (with pipeline layout handle)
    // ============================================================================

    void RHICmd::BindDescriptorSet(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                   RHIDescriptorSetHandle set, uint32_t setIndex)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout vkLayout = registry.GetPipelineLayout(layout);
        VkDescriptorSet vkSet = registry.GetDescriptorSet(set);
        if (vkCmd == VK_NULL_HANDLE || vkLayout == VK_NULL_HANDLE || vkSet == VK_NULL_HANDLE) return;

        vkCmdBindDescriptorSets(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, vkLayout, setIndex, 1, &vkSet, 0, nullptr);
    }

    void RHICmd::BindDescriptorSetRaw(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                      void* descriptorSet, uint32_t setIndex)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        VkPipelineLayout vkLayout = registry.GetPipelineLayout(layout);
        VkDescriptorSet vkSet = static_cast<VkDescriptorSet>(descriptorSet);
        if (vkCmd == VK_NULL_HANDLE || vkLayout == VK_NULL_HANDLE || vkSet == VK_NULL_HANDLE) return;

        vkCmdBindDescriptorSets(vkCmd, VK_PIPELINE_BIND_POINT_GRAPHICS, vkLayout, setIndex, 1, &vkSet, 0, nullptr);
    }

}
