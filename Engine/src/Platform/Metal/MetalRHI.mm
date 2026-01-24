#include "ggpch.h"
#include "MetalRHI.h"
#include "MetalContext.h"
#include "MetalUtils.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/RHI/RHICommandBuffer.h"
#include "GGEngine/RHI/RHISpecifications.h"
#include "GGEngine/Renderer/VertexLayout.h"

#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

// SPIRV-Cross for shader compilation
#include <spirv_cross/spirv_msl.hpp>

namespace GGEngine {

    // ============================================================================
    // Metal Type Conversions
    // ============================================================================

    MTLPrimitiveType ToMetal(PrimitiveTopology topology)
    {
        switch (topology)
        {
            case PrimitiveTopology::PointList:     return MTLPrimitiveTypePoint;
            case PrimitiveTopology::LineList:      return MTLPrimitiveTypeLine;
            case PrimitiveTopology::LineStrip:     return MTLPrimitiveTypeLineStrip;
            case PrimitiveTopology::TriangleList:  return MTLPrimitiveTypeTriangle;
            case PrimitiveTopology::TriangleStrip: return MTLPrimitiveTypeTriangleStrip;
            case PrimitiveTopology::TriangleFan:   return MTLPrimitiveTypeTriangle; // Emulated
            default:                               return MTLPrimitiveTypeTriangle;
        }
    }

    MTLTriangleFillMode ToMetal(PolygonMode mode)
    {
        switch (mode)
        {
            case PolygonMode::Fill:  return MTLTriangleFillModeFill;
            case PolygonMode::Line:  return MTLTriangleFillModeLines;
            case PolygonMode::Point: return MTLTriangleFillModeFill; // Not supported
            default:                 return MTLTriangleFillModeFill;
        }
    }

    MTLCullMode ToMetal(CullMode mode)
    {
        switch (mode)
        {
            case CullMode::None:         return MTLCullModeNone;
            case CullMode::Front:        return MTLCullModeFront;
            case CullMode::Back:         return MTLCullModeBack;
            case CullMode::FrontAndBack: return MTLCullModeNone; // Not directly supported
            default:                     return MTLCullModeNone;
        }
    }

    MTLWinding ToMetal(FrontFace face)
    {
        switch (face)
        {
            case FrontFace::CounterClockwise: return MTLWindingCounterClockwise;
            case FrontFace::Clockwise:        return MTLWindingClockwise;
            default:                          return MTLWindingClockwise;
        }
    }

    MTLCompareFunction ToMetal(CompareOp op)
    {
        switch (op)
        {
            case CompareOp::Never:          return MTLCompareFunctionNever;
            case CompareOp::Less:           return MTLCompareFunctionLess;
            case CompareOp::Equal:          return MTLCompareFunctionEqual;
            case CompareOp::LessOrEqual:    return MTLCompareFunctionLessEqual;
            case CompareOp::Greater:        return MTLCompareFunctionGreater;
            case CompareOp::NotEqual:       return MTLCompareFunctionNotEqual;
            case CompareOp::GreaterOrEqual: return MTLCompareFunctionGreaterEqual;
            case CompareOp::Always:         return MTLCompareFunctionAlways;
            default:                        return MTLCompareFunctionLess;
        }
    }

    NSUInteger ToMetalSampleCount(SampleCount count)
    {
        return static_cast<NSUInteger>(count);
    }

    MTLPixelFormat ToMetal(TextureFormat format)
    {
        switch (format)
        {
            case TextureFormat::Undefined:          return MTLPixelFormatInvalid;
            case TextureFormat::R8_UNORM:           return MTLPixelFormatR8Unorm;
            case TextureFormat::R8_SNORM:           return MTLPixelFormatR8Snorm;
            case TextureFormat::R8_UINT:            return MTLPixelFormatR8Uint;
            case TextureFormat::R8_SINT:            return MTLPixelFormatR8Sint;
            case TextureFormat::R8G8_UNORM:         return MTLPixelFormatRG8Unorm;
            case TextureFormat::R8G8_SNORM:         return MTLPixelFormatRG8Snorm;
            case TextureFormat::R8G8_UINT:          return MTLPixelFormatRG8Uint;
            case TextureFormat::R8G8_SINT:          return MTLPixelFormatRG8Sint;
            case TextureFormat::R8G8B8_UNORM:       return MTLPixelFormatRGBA8Unorm; // No RGB8
            case TextureFormat::R8G8B8_SRGB:        return MTLPixelFormatRGBA8Unorm_sRGB;
            case TextureFormat::R8G8B8A8_UNORM:     return MTLPixelFormatRGBA8Unorm;
            case TextureFormat::R8G8B8A8_SNORM:     return MTLPixelFormatRGBA8Snorm;
            case TextureFormat::R8G8B8A8_UINT:      return MTLPixelFormatRGBA8Uint;
            case TextureFormat::R8G8B8A8_SINT:      return MTLPixelFormatRGBA8Sint;
            case TextureFormat::R8G8B8A8_SRGB:      return MTLPixelFormatRGBA8Unorm_sRGB;
            case TextureFormat::B8G8R8A8_UNORM:     return MTLPixelFormatBGRA8Unorm;
            case TextureFormat::B8G8R8A8_SRGB:      return MTLPixelFormatBGRA8Unorm_sRGB;
            case TextureFormat::R16_UNORM:          return MTLPixelFormatR16Unorm;
            case TextureFormat::R16_SNORM:          return MTLPixelFormatR16Snorm;
            case TextureFormat::R16_UINT:           return MTLPixelFormatR16Uint;
            case TextureFormat::R16_SINT:           return MTLPixelFormatR16Sint;
            case TextureFormat::R16_SFLOAT:         return MTLPixelFormatR16Float;
            case TextureFormat::R16G16_UNORM:       return MTLPixelFormatRG16Unorm;
            case TextureFormat::R16G16_SNORM:       return MTLPixelFormatRG16Snorm;
            case TextureFormat::R16G16_UINT:        return MTLPixelFormatRG16Uint;
            case TextureFormat::R16G16_SINT:        return MTLPixelFormatRG16Sint;
            case TextureFormat::R16G16_SFLOAT:      return MTLPixelFormatRG16Float;
            case TextureFormat::R16G16B16A16_UNORM: return MTLPixelFormatRGBA16Unorm;
            case TextureFormat::R16G16B16A16_SNORM: return MTLPixelFormatRGBA16Snorm;
            case TextureFormat::R16G16B16A16_UINT:  return MTLPixelFormatRGBA16Uint;
            case TextureFormat::R16G16B16A16_SINT:  return MTLPixelFormatRGBA16Sint;
            case TextureFormat::R16G16B16A16_SFLOAT:return MTLPixelFormatRGBA16Float;
            case TextureFormat::R32_UINT:           return MTLPixelFormatR32Uint;
            case TextureFormat::R32_SINT:           return MTLPixelFormatR32Sint;
            case TextureFormat::R32_SFLOAT:         return MTLPixelFormatR32Float;
            case TextureFormat::R32G32_UINT:        return MTLPixelFormatRG32Uint;
            case TextureFormat::R32G32_SINT:        return MTLPixelFormatRG32Sint;
            case TextureFormat::R32G32_SFLOAT:      return MTLPixelFormatRG32Float;
            case TextureFormat::R32G32B32_UINT:     return MTLPixelFormatInvalid; // No RGB32
            case TextureFormat::R32G32B32_SINT:     return MTLPixelFormatInvalid;
            case TextureFormat::R32G32B32_SFLOAT:   return MTLPixelFormatInvalid;
            case TextureFormat::R32G32B32A32_UINT:  return MTLPixelFormatRGBA32Uint;
            case TextureFormat::R32G32B32A32_SINT:  return MTLPixelFormatRGBA32Sint;
            case TextureFormat::R32G32B32A32_SFLOAT:return MTLPixelFormatRGBA32Float;
            case TextureFormat::D16_UNORM:          return MTLPixelFormatDepth16Unorm;
            case TextureFormat::D32_SFLOAT:         return MTLPixelFormatDepth32Float;
            case TextureFormat::D24_UNORM_S8_UINT:  return MTLPixelFormatDepth24Unorm_Stencil8;
            case TextureFormat::D32_SFLOAT_S8_UINT: return MTLPixelFormatDepth32Float_Stencil8;
            case TextureFormat::S8_UINT:            return MTLPixelFormatStencil8;
            case TextureFormat::BC1_RGB_UNORM:      return MTLPixelFormatBC1_RGBA;
            case TextureFormat::BC1_RGB_SRGB:       return MTLPixelFormatBC1_RGBA_sRGB;
            case TextureFormat::BC1_RGBA_UNORM:     return MTLPixelFormatBC1_RGBA;
            case TextureFormat::BC1_RGBA_SRGB:      return MTLPixelFormatBC1_RGBA_sRGB;
            case TextureFormat::BC2_UNORM:          return MTLPixelFormatBC2_RGBA;
            case TextureFormat::BC2_SRGB:           return MTLPixelFormatBC2_RGBA_sRGB;
            case TextureFormat::BC3_UNORM:          return MTLPixelFormatBC3_RGBA;
            case TextureFormat::BC3_SRGB:           return MTLPixelFormatBC3_RGBA_sRGB;
            case TextureFormat::BC4_UNORM:          return MTLPixelFormatBC4_RUnorm;
            case TextureFormat::BC4_SNORM:          return MTLPixelFormatBC4_RSnorm;
            case TextureFormat::BC5_UNORM:          return MTLPixelFormatBC5_RGUnorm;
            case TextureFormat::BC5_SNORM:          return MTLPixelFormatBC5_RGSnorm;
            case TextureFormat::BC6H_UFLOAT:        return MTLPixelFormatBC6H_RGBUfloat;
            case TextureFormat::BC6H_SFLOAT:        return MTLPixelFormatBC6H_RGBFloat;
            case TextureFormat::BC7_UNORM:          return MTLPixelFormatBC7_RGBAUnorm;
            case TextureFormat::BC7_SRGB:           return MTLPixelFormatBC7_RGBAUnorm_sRGB;
            default:                                return MTLPixelFormatInvalid;
        }
    }

    MTLVertexFormat ToMetalVertexFormat(TextureFormat format)
    {
        switch (format)
        {
            case TextureFormat::R8_UNORM:           return MTLVertexFormatUCharNormalized;
            case TextureFormat::R8_UINT:            return MTLVertexFormatUChar;
            case TextureFormat::R8_SINT:            return MTLVertexFormatChar;
            case TextureFormat::R8G8_UNORM:         return MTLVertexFormatUChar2Normalized;
            case TextureFormat::R8G8_UINT:          return MTLVertexFormatUChar2;
            case TextureFormat::R8G8B8A8_UNORM:     return MTLVertexFormatUChar4Normalized;
            case TextureFormat::R8G8B8A8_UINT:      return MTLVertexFormatUChar4;
            case TextureFormat::R16_SFLOAT:         return MTLVertexFormatHalf;
            case TextureFormat::R16G16_SFLOAT:      return MTLVertexFormatHalf2;
            case TextureFormat::R16G16B16A16_SFLOAT:return MTLVertexFormatHalf4;
            case TextureFormat::R32_SFLOAT:         return MTLVertexFormatFloat;
            case TextureFormat::R32G32_SFLOAT:      return MTLVertexFormatFloat2;
            case TextureFormat::R32G32B32_SFLOAT:   return MTLVertexFormatFloat3;
            case TextureFormat::R32G32B32A32_SFLOAT:return MTLVertexFormatFloat4;
            case TextureFormat::R32_UINT:           return MTLVertexFormatUInt;
            case TextureFormat::R32G32_UINT:        return MTLVertexFormatUInt2;
            case TextureFormat::R32G32B32_UINT:     return MTLVertexFormatUInt3;
            case TextureFormat::R32G32B32A32_UINT:  return MTLVertexFormatUInt4;
            case TextureFormat::R32_SINT:           return MTLVertexFormatInt;
            case TextureFormat::R32G32_SINT:        return MTLVertexFormatInt2;
            case TextureFormat::R32G32B32_SINT:     return MTLVertexFormatInt3;
            case TextureFormat::R32G32B32A32_SINT:  return MTLVertexFormatInt4;
            default:                                return MTLVertexFormatInvalid;
        }
    }

    TextureFormat FromMetal(MTLPixelFormat format)
    {
        switch (format)
        {
            case MTLPixelFormatR8Unorm:              return TextureFormat::R8_UNORM;
            case MTLPixelFormatRGBA8Unorm:           return TextureFormat::R8G8B8A8_UNORM;
            case MTLPixelFormatRGBA8Unorm_sRGB:      return TextureFormat::R8G8B8A8_SRGB;
            case MTLPixelFormatBGRA8Unorm:           return TextureFormat::B8G8R8A8_UNORM;
            case MTLPixelFormatBGRA8Unorm_sRGB:      return TextureFormat::B8G8R8A8_SRGB;
            case MTLPixelFormatDepth32Float:         return TextureFormat::D32_SFLOAT;
            case MTLPixelFormatDepth32Float_Stencil8:return TextureFormat::D32_SFLOAT_S8_UINT;
            default:                                 return TextureFormat::Undefined;
        }
    }

    MTLSamplerMinMagFilter ToMetal(Filter filter)
    {
        switch (filter)
        {
            case Filter::Nearest: return MTLSamplerMinMagFilterNearest;
            case Filter::Linear:  return MTLSamplerMinMagFilterLinear;
            default:              return MTLSamplerMinMagFilterNearest;
        }
    }

    MTLSamplerMipFilter ToMetalMipFilter(MipmapMode mode)
    {
        switch (mode)
        {
            case MipmapMode::Nearest: return MTLSamplerMipFilterNearest;
            case MipmapMode::Linear:  return MTLSamplerMipFilterLinear;
            default:                  return MTLSamplerMipFilterNearest;
        }
    }

    MTLSamplerAddressMode ToMetal(AddressMode mode)
    {
        switch (mode)
        {
            case AddressMode::Repeat:           return MTLSamplerAddressModeRepeat;
            case AddressMode::MirroredRepeat:   return MTLSamplerAddressModeMirrorRepeat;
            case AddressMode::ClampToEdge:      return MTLSamplerAddressModeClampToEdge;
            case AddressMode::ClampToBorder:    return MTLSamplerAddressModeClampToBorderColor;
            case AddressMode::MirrorClampToEdge:return MTLSamplerAddressModeMirrorClampToEdge;
            default:                            return MTLSamplerAddressModeRepeat;
        }
    }

    MTLIndexType ToMetal(IndexType type)
    {
        switch (type)
        {
            case IndexType::UInt16: return MTLIndexTypeUInt16;
            case IndexType::UInt32: return MTLIndexTypeUInt32;
            default:                return MTLIndexTypeUInt32;
        }
    }

    MTLBlendFactor ToMetal(BlendFactor factor)
    {
        switch (factor)
        {
            case BlendFactor::Zero:                  return MTLBlendFactorZero;
            case BlendFactor::One:                   return MTLBlendFactorOne;
            case BlendFactor::SrcColor:              return MTLBlendFactorSourceColor;
            case BlendFactor::OneMinusSrcColor:      return MTLBlendFactorOneMinusSourceColor;
            case BlendFactor::DstColor:              return MTLBlendFactorDestinationColor;
            case BlendFactor::OneMinusDstColor:      return MTLBlendFactorOneMinusDestinationColor;
            case BlendFactor::SrcAlpha:              return MTLBlendFactorSourceAlpha;
            case BlendFactor::OneMinusSrcAlpha:      return MTLBlendFactorOneMinusSourceAlpha;
            case BlendFactor::DstAlpha:              return MTLBlendFactorDestinationAlpha;
            case BlendFactor::OneMinusDstAlpha:      return MTLBlendFactorOneMinusDestinationAlpha;
            case BlendFactor::ConstantColor:         return MTLBlendFactorBlendColor;
            case BlendFactor::OneMinusConstantColor: return MTLBlendFactorOneMinusBlendColor;
            case BlendFactor::ConstantAlpha:         return MTLBlendFactorBlendAlpha;
            case BlendFactor::OneMinusConstantAlpha: return MTLBlendFactorOneMinusBlendAlpha;
            case BlendFactor::SrcAlphaSaturate:      return MTLBlendFactorSourceAlphaSaturated;
            default:                                 return MTLBlendFactorOne;
        }
    }

    MTLBlendOperation ToMetal(BlendOp op)
    {
        switch (op)
        {
            case BlendOp::Add:             return MTLBlendOperationAdd;
            case BlendOp::Subtract:        return MTLBlendOperationSubtract;
            case BlendOp::ReverseSubtract: return MTLBlendOperationReverseSubtract;
            case BlendOp::Min:             return MTLBlendOperationMin;
            case BlendOp::Max:             return MTLBlendOperationMax;
            default:                       return MTLBlendOperationAdd;
        }
    }

    MTLColorWriteMask ToMetal(ColorComponent components)
    {
        MTLColorWriteMask mask = MTLColorWriteMaskNone;
        if ((static_cast<uint8_t>(components) & static_cast<uint8_t>(ColorComponent::R)) != 0)
            mask |= MTLColorWriteMaskRed;
        if ((static_cast<uint8_t>(components) & static_cast<uint8_t>(ColorComponent::G)) != 0)
            mask |= MTLColorWriteMaskGreen;
        if ((static_cast<uint8_t>(components) & static_cast<uint8_t>(ColorComponent::B)) != 0)
            mask |= MTLColorWriteMaskBlue;
        if ((static_cast<uint8_t>(components) & static_cast<uint8_t>(ColorComponent::A)) != 0)
            mask |= MTLColorWriteMaskAlpha;
        return mask;
    }

    MTLLoadAction ToMetal(LoadOp op)
    {
        switch (op)
        {
            case LoadOp::Load:     return MTLLoadActionLoad;
            case LoadOp::Clear:    return MTLLoadActionClear;
            case LoadOp::DontCare: return MTLLoadActionDontCare;
            default:               return MTLLoadActionDontCare;
        }
    }

    MTLStoreAction ToMetal(StoreOp op)
    {
        switch (op)
        {
            case StoreOp::Store:    return MTLStoreActionStore;
            case StoreOp::DontCare: return MTLStoreActionDontCare;
            default:                return MTLStoreActionStore;
        }
    }

    // ============================================================================
    // Metal Resource Registry Implementation
    // ============================================================================

    MetalResourceRegistry& MetalResourceRegistry::Get()
    {
        static MetalResourceRegistry instance;
        return instance;
    }

    uint64_t MetalResourceRegistry::GenerateId()
    {
        return m_NextId++;
    }

    // Pipeline
    RHIPipelineHandle MetalResourceRegistry::RegisterPipeline(const PipelineData& data)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Pipelines[id] = data;
        return RHIPipelineHandle{id};
    }

    void MetalResourceRegistry::UnregisterPipeline(RHIPipelineHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Pipelines.erase(handle.id);
    }

    MetalResourceRegistry::PipelineData MetalResourceRegistry::GetPipelineData(RHIPipelineHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_Pipelines.find(handle.id);
        if (it != m_Pipelines.end())
            return it->second;
        return PipelineData{};
    }

    // Pipeline Layout
    RHIPipelineLayoutHandle MetalResourceRegistry::RegisterPipelineLayout(const PipelineLayoutData& data)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_PipelineLayouts[id] = data;
        return RHIPipelineLayoutHandle{id};
    }

    void MetalResourceRegistry::UnregisterPipelineLayout(RHIPipelineLayoutHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_PipelineLayouts.erase(handle.id);
    }

    MetalResourceRegistry::PipelineLayoutData MetalResourceRegistry::GetPipelineLayoutData(RHIPipelineLayoutHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_PipelineLayouts.find(handle.id);
        if (it != m_PipelineLayouts.end())
            return it->second;
        return PipelineLayoutData{};
    }

    // Render Pass
    RHIRenderPassHandle MetalResourceRegistry::RegisterRenderPass(const RenderPassData& data)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_RenderPasses[id] = data;
        return RHIRenderPassHandle{id};
    }

    void MetalResourceRegistry::UnregisterRenderPass(RHIRenderPassHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_RenderPasses.erase(handle.id);
    }

    MetalResourceRegistry::RenderPassData MetalResourceRegistry::GetRenderPassData(RHIRenderPassHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_RenderPasses.find(handle.id);
        if (it != m_RenderPasses.end())
            return it->second;
        return RenderPassData{};
    }

    // Buffer
    RHIBufferHandle MetalResourceRegistry::RegisterBuffer(const BufferData& data)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Buffers[id] = data;
        return RHIBufferHandle{id};
    }

    void MetalResourceRegistry::UnregisterBuffer(RHIBufferHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Buffers.erase(handle.id);
    }

    MetalResourceRegistry::BufferData MetalResourceRegistry::GetBufferData(RHIBufferHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_Buffers.find(handle.id);
        if (it != m_Buffers.end())
            return it->second;
        return BufferData{};
    }

    // Texture
    RHITextureHandle MetalResourceRegistry::RegisterTexture(const TextureData& data)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Textures[id] = data;
        return RHITextureHandle{id};
    }

    void MetalResourceRegistry::UnregisterTexture(RHITextureHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Textures.erase(handle.id);
    }

    MetalResourceRegistry::TextureData MetalResourceRegistry::GetTextureData(RHITextureHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_Textures.find(handle.id);
        if (it != m_Textures.end())
            return it->second;
        return TextureData{};
    }

    // Sampler
    RHISamplerHandle MetalResourceRegistry::RegisterSampler(id<MTLSamplerState> sampler)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_Samplers[id] = sampler;
        return RHISamplerHandle{id};
    }

    void MetalResourceRegistry::UnregisterSampler(RHISamplerHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Samplers.erase(handle.id);
    }

    id<MTLSamplerState> MetalResourceRegistry::GetSampler(RHISamplerHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_Samplers.find(handle.id);
        if (it != m_Samplers.end())
            return it->second;
        return nil;
    }

    // Shader Module
    RHIShaderModuleHandle MetalResourceRegistry::RegisterShaderModule(const ShaderModuleData& data)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_ShaderModules[id] = data;
        return RHIShaderModuleHandle{id};
    }

    void MetalResourceRegistry::UnregisterShaderModule(RHIShaderModuleHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_ShaderModules.erase(handle.id);
    }

    MetalResourceRegistry::ShaderModuleData MetalResourceRegistry::GetShaderModuleData(RHIShaderModuleHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_ShaderModules.find(handle.id);
        if (it != m_ShaderModules.end())
            return it->second;
        return ShaderModuleData{};
    }

    // Descriptor Set Layout
    RHIDescriptorSetLayoutHandle MetalResourceRegistry::RegisterDescriptorSetLayout(const DescriptorSetLayoutData& data)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_DescriptorSetLayouts[id] = data;
        return RHIDescriptorSetLayoutHandle{id};
    }

    void MetalResourceRegistry::UnregisterDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_DescriptorSetLayouts.erase(handle.id);
    }

    MetalResourceRegistry::DescriptorSetLayoutData MetalResourceRegistry::GetDescriptorSetLayoutData(RHIDescriptorSetLayoutHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_DescriptorSetLayouts.find(handle.id);
        if (it != m_DescriptorSetLayouts.end())
            return it->second;
        return DescriptorSetLayoutData{};
    }

    // Descriptor Set
    RHIDescriptorSetHandle MetalResourceRegistry::RegisterDescriptorSet(const DescriptorSetData& data)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        uint64_t id = GenerateId();
        m_DescriptorSets[id] = data;
        return RHIDescriptorSetHandle{id};
    }

    void MetalResourceRegistry::UnregisterDescriptorSet(RHIDescriptorSetHandle handle)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_DescriptorSets.erase(handle.id);
    }

    MetalResourceRegistry::DescriptorSetData MetalResourceRegistry::GetDescriptorSetData(RHIDescriptorSetHandle handle) const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        auto it = m_DescriptorSets.find(handle.id);
        if (it != m_DescriptorSets.end())
            return it->second;
        return DescriptorSetData{};
    }

    // Command Buffer
    void MetalResourceRegistry::SetCurrentCommandBuffer(uint32_t frameIndex, id<MTLCommandBuffer> cmd)
    {
        if (frameIndex >= MaxFramesInFlight) return;
        m_CommandBuffers[frameIndex] = cmd;
        m_CommandBufferHandleIds[frameIndex] = (frameIndex == 0) ? 0xFFFF0001 : 0xFFFF0002;
    }

    RHICommandBufferHandle MetalResourceRegistry::GetCurrentCommandBufferHandle(uint32_t frameIndex) const
    {
        if (frameIndex >= MaxFramesInFlight) return NullCommandBuffer;
        return RHICommandBufferHandle{m_CommandBufferHandleIds[frameIndex]};
    }

    id<MTLCommandBuffer> MetalResourceRegistry::GetCommandBuffer(RHICommandBufferHandle handle) const
    {
        if (handle.id == ImmediateCommandBufferHandleId)
            return m_ImmediateCommandBuffer;
        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            if (m_CommandBufferHandleIds[i] == handle.id)
                return m_CommandBuffers[i];
        }
        return nil;
    }

    void MetalResourceRegistry::SetImmediateCommandBuffer(id<MTLCommandBuffer> cmd)
    {
        m_ImmediateCommandBuffer = cmd;
    }

    RHICommandBufferHandle MetalResourceRegistry::GetImmediateCommandBufferHandle() const
    {
        return RHICommandBufferHandle{ImmediateCommandBufferHandleId};
    }

    void MetalResourceRegistry::Clear()
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Pipelines.clear();
        m_PipelineLayouts.clear();
        m_RenderPasses.clear();
        m_Buffers.clear();
        m_Textures.clear();
        m_Samplers.clear();
        m_ShaderModules.clear();
        m_DescriptorSetLayouts.clear();
        m_DescriptorSets.clear();
    }

    // ============================================================================
    // RHIDevice Implementation (Metal Backend)
    // ============================================================================

    RHIDevice& RHIDevice::Get()
    {
        static RHIDevice instance;
        return instance;
    }

    void RHIDevice::Init(void* windowHandle)
    {
        MetalContext::Get().Init(static_cast<GLFWwindow*>(windowHandle));

        // Create and register swapchain render pass
        MetalResourceRegistry::RenderPassData rpData;
        rpData.colorFormat = TextureFormat::B8G8R8A8_UNORM;
        rpData.colorLoadOp = LoadOp::Clear;
        rpData.colorStoreOp = StoreOp::Store;
        rpData.width = MetalContext::Get().GetSwapchainWidth();
        rpData.height = MetalContext::Get().GetSwapchainHeight();
        m_SwapchainRenderPassHandle = MetalResourceRegistry::Get().RegisterRenderPass(rpData);

        m_Initialized = true;
    }

    void RHIDevice::Shutdown()
    {
        MetalResourceRegistry::Get().Clear();
        MetalContext::Get().Shutdown();
        m_Initialized = false;
    }

    void RHIDevice::BeginFrame()
    {
        MetalContext::Get().BeginFrame();
    }

    void RHIDevice::EndFrame()
    {
        MetalContext::Get().EndFrame();
    }

    void RHIDevice::BeginSwapchainRenderPass()
    {
        MetalContext::Get().BeginSwapchainRenderPass();
    }

    RHICommandBufferHandle RHIDevice::GetCurrentCommandBuffer() const
    {
        return MetalResourceRegistry::Get().GetCurrentCommandBufferHandle(
            MetalContext::Get().GetCurrentFrameIndex());
    }

    RHIRenderPassHandle RHIDevice::GetSwapchainRenderPass() const
    {
        return m_SwapchainRenderPassHandle;
    }

    uint32_t RHIDevice::GetSwapchainWidth() const
    {
        return MetalContext::Get().GetSwapchainWidth();
    }

    uint32_t RHIDevice::GetSwapchainHeight() const
    {
        return MetalContext::Get().GetSwapchainHeight();
    }

    uint32_t RHIDevice::GetCurrentFrameIndex() const
    {
        return MetalContext::Get().GetCurrentFrameIndex();
    }

    void RHIDevice::ImmediateSubmit(const std::function<void(RHICommandBufferHandle)>& func)
    {
        MetalContext::Get().ImmediateSubmit([&func](id<MTLCommandBuffer> cmd) {
            func(MetalResourceRegistry::Get().GetImmediateCommandBufferHandle());
        });
    }

    void RHIDevice::WaitIdle()
    {
        MetalContext::Get().WaitIdle();
    }

    void RHIDevice::OnWindowResize(uint32_t width, uint32_t height)
    {
        MetalContext::Get().OnWindowResize(width, height);
    }

    void RHIDevice::SetVSync(bool enabled)
    {
        MetalContext::Get().SetVSync(enabled);
    }

    bool RHIDevice::IsVSync() const
    {
        return MetalContext::Get().IsVSync();
    }

    // ========================================================================
    // Buffer Management
    // ========================================================================

    RHIBufferHandle RHIDevice::CreateBuffer(const RHIBufferSpecification& spec)
    {
        auto result = TryCreateBuffer(spec);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateBuffer failed: {}", result.Error());
            return NullBuffer;
        }
        return result.Value();
    }

    Result<RHIBufferHandle> RHIDevice::TryCreateBuffer(const RHIBufferSpecification& spec)
    {
        id<MTLDevice> device = MetalContext::Get().GetDevice();

        MTLResourceOptions options = spec.cpuVisible
            ? MTLResourceStorageModeShared
            : MTLResourceStorageModePrivate;

        id<MTLBuffer> buffer = [device newBufferWithLength:spec.size options:options];
        if (!buffer)
        {
            return Result<RHIBufferHandle>::Err("Failed to create MTLBuffer");
        }

        if (!spec.debugName.empty())
        {
            buffer.label = [NSString stringWithUTF8String:spec.debugName.c_str()];
        }

        MetalResourceRegistry::BufferData data;
        data.buffer = buffer;
        data.size = spec.size;
        data.cpuVisible = spec.cpuVisible;

        return Result<RHIBufferHandle>::Ok(MetalResourceRegistry::Get().RegisterBuffer(data));
    }

    void RHIDevice::DestroyBuffer(RHIBufferHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterBuffer(handle);
    }

    void* RHIDevice::MapBuffer(RHIBufferHandle handle)
    {
        auto data = MetalResourceRegistry::Get().GetBufferData(handle);
        if (!data.buffer || !data.cpuVisible)
            return nullptr;
        return [data.buffer contents];
    }

    void RHIDevice::UnmapBuffer(RHIBufferHandle handle)
    {
        // Metal shared memory doesn't need explicit unmap
    }

    void RHIDevice::FlushBuffer(RHIBufferHandle handle, uint64_t offset, uint64_t size)
    {
        auto data = MetalResourceRegistry::Get().GetBufferData(handle);
        if (!data.buffer) return;

        // For managed resources on macOS, synchronize to GPU
        // Shared resources don't need this
    }

    void RHIDevice::UploadBufferData(RHIBufferHandle handle, const void* srcData, uint64_t size, uint64_t offset)
    {
        auto data = MetalResourceRegistry::Get().GetBufferData(handle);
        if (!data.buffer) return;

        if (data.cpuVisible)
        {
            memcpy(static_cast<uint8_t*>([data.buffer contents]) + offset, srcData, size);
        }
        else
        {
            // Create staging buffer and blit
            id<MTLDevice> device = MetalContext::Get().GetDevice();
            id<MTLBuffer> staging = [device newBufferWithBytes:srcData
                                                        length:size
                                                       options:MTLResourceStorageModeShared];

            MetalContext::Get().ImmediateSubmit([&](id<MTLCommandBuffer> cmd) {
                id<MTLBlitCommandEncoder> blit = [cmd blitCommandEncoder];
                [blit copyFromBuffer:staging sourceOffset:0
                            toBuffer:data.buffer destinationOffset:offset
                                size:size];
                [blit endEncoding];
            });
        }
    }

    // ========================================================================
    // Texture Management
    // ========================================================================

    RHITextureHandle RHIDevice::CreateTexture(const RHITextureSpecification& spec)
    {
        auto result = TryCreateTexture(spec);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateTexture failed: {}", result.Error());
            return NullTexture;
        }
        return result.Value();
    }

    Result<RHITextureHandle> RHIDevice::TryCreateTexture(const RHITextureSpecification& spec)
    {
        id<MTLDevice> device = MetalContext::Get().GetDevice();

        MTLTextureDescriptor* desc = [[MTLTextureDescriptor alloc] init];
        desc.textureType = MTLTextureType2D;
        desc.pixelFormat = ToMetal(spec.format);
        desc.width = spec.width;
        desc.height = spec.height;
        desc.depth = 1;
        desc.mipmapLevelCount = spec.mipLevels > 0 ? spec.mipLevels : 1;
        desc.arrayLength = spec.arrayLayers > 0 ? spec.arrayLayers : 1;
        desc.sampleCount = ToMetalSampleCount(spec.samples);
        desc.storageMode = MTLStorageModePrivate;
        desc.usage = MTLTextureUsageShaderRead;

        if (IsDepthFormat(spec.format))
        {
            desc.usage |= MTLTextureUsageRenderTarget;
        }

        id<MTLTexture> texture = [device newTextureWithDescriptor:desc];
        if (!texture)
        {
            return Result<RHITextureHandle>::Err("Failed to create MTLTexture");
        }

        if (!spec.debugName.empty())
        {
            texture.label = [NSString stringWithUTF8String:spec.debugName.c_str()];
        }

        MetalResourceRegistry::TextureData data;
        data.texture = texture;
        data.width = spec.width;
        data.height = spec.height;
        data.format = spec.format;

        return Result<RHITextureHandle>::Ok(MetalResourceRegistry::Get().RegisterTexture(data));
    }

    void RHIDevice::DestroyTexture(RHITextureHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterTexture(handle);
    }

    void RHIDevice::UploadTextureData(RHITextureHandle handle, const void* pixels, uint64_t size)
    {
        auto data = MetalResourceRegistry::Get().GetTextureData(handle);
        if (!data.texture) return;

        MTLRegion region = MTLRegionMake2D(0, 0, data.width, data.height);
        NSUInteger bytesPerRow = size / data.height;

        // Create staging buffer
        id<MTLDevice> device = MetalContext::Get().GetDevice();
        id<MTLBuffer> staging = [device newBufferWithBytes:pixels
                                                    length:size
                                                   options:MTLResourceStorageModeShared];

        MetalContext::Get().ImmediateSubmit([&](id<MTLCommandBuffer> cmd) {
            id<MTLBlitCommandEncoder> blit = [cmd blitCommandEncoder];
            [blit copyFromBuffer:staging
                    sourceOffset:0
               sourceBytesPerRow:bytesPerRow
             sourceBytesPerImage:size
                      sourceSize:MTLSizeMake(data.width, data.height, 1)
                       toTexture:data.texture
                destinationSlice:0
                destinationLevel:0
               destinationOrigin:MTLOriginMake(0, 0, 0)];
            [blit endEncoding];
        });
    }

    uint32_t RHIDevice::GetTextureWidth(RHITextureHandle handle) const
    {
        return MetalResourceRegistry::Get().GetTextureData(handle).width;
    }

    uint32_t RHIDevice::GetTextureHeight(RHITextureHandle handle) const
    {
        return MetalResourceRegistry::Get().GetTextureData(handle).height;
    }

    // ========================================================================
    // Sampler Management
    // ========================================================================

    RHISamplerHandle RHIDevice::CreateSampler(const RHISamplerSpecification& spec)
    {
        id<MTLDevice> device = MetalContext::Get().GetDevice();

        MTLSamplerDescriptor* desc = [[MTLSamplerDescriptor alloc] init];
        desc.minFilter = ToMetal(spec.minFilter);
        desc.magFilter = ToMetal(spec.magFilter);
        desc.mipFilter = ToMetalMipFilter(spec.mipmapMode);
        desc.sAddressMode = ToMetal(spec.addressModeU);
        desc.tAddressMode = ToMetal(spec.addressModeV);
        desc.rAddressMode = ToMetal(spec.addressModeW);
        desc.lodMinClamp = spec.minLod;
        desc.lodMaxClamp = spec.maxLod;
        desc.maxAnisotropy = spec.anisotropyEnable ? static_cast<NSUInteger>(spec.maxAnisotropy) : 1;
        desc.compareFunction = spec.compareEnable ? ToMetal(spec.compareOp) : MTLCompareFunctionNever;

        id<MTLSamplerState> sampler = [device newSamplerStateWithDescriptor:desc];
        MTL_CHECK_RETURN_VAL(sampler != nil, "Failed to create sampler", NullSampler);

        return MetalResourceRegistry::Get().RegisterSampler(sampler);
    }

    void RHIDevice::DestroySampler(RHISamplerHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterSampler(handle);
    }

    // ========================================================================
    // Shader Management
    // ========================================================================

    RHIShaderModuleHandle RHIDevice::CreateShaderModule(ShaderStage stage, const std::vector<char>& spirvCode)
    {
        return CreateShaderModule(stage, spirvCode.data(), spirvCode.size());
    }

    RHIShaderModuleHandle RHIDevice::CreateShaderModule(ShaderStage stage, const void* spirvData, size_t spirvSize)
    {
        auto result = TryCreateShaderModule(stage, spirvData, spirvSize);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateShaderModule failed: {}", result.Error());
            return NullShaderModule;
        }
        return result.Value();
    }

    Result<RHIShaderModuleHandle> RHIDevice::TryCreateShaderModule(ShaderStage stage, const std::vector<char>& spirvCode)
    {
        return TryCreateShaderModule(stage, spirvCode.data(), spirvCode.size());
    }

    Result<RHIShaderModuleHandle> RHIDevice::TryCreateShaderModule(ShaderStage stage, const void* spirvData, size_t spirvSize)
    {
        // Compile SPIR-V to MSL using SPIRV-Cross
        std::vector<uint32_t> spirv(reinterpret_cast<const uint32_t*>(spirvData),
                                    reinterpret_cast<const uint32_t*>(spirvData) + spirvSize / sizeof(uint32_t));

        spirv_cross::CompilerMSL msl(std::move(spirv));

        spirv_cross::CompilerMSL::Options options;
        options.platform = spirv_cross::CompilerMSL::Options::macOS;
        options.msl_version = spirv_cross::CompilerMSL::Options::make_msl_version(3, 0);
        options.argument_buffers = true;
        options.force_active_argument_buffer_resources = true;
        msl.set_msl_options(options);

        std::string mslSource;
        try
        {
            mslSource = msl.compile();
        }
        catch (const spirv_cross::CompilerError& e)
        {
            return Result<RHIShaderModuleHandle>::Err(std::string("SPIRV-Cross compilation failed: ") + e.what());
        }

        // Compile MSL to MTLLibrary
        id<MTLDevice> device = MetalContext::Get().GetDevice();
        NSString* source = [NSString stringWithUTF8String:mslSource.c_str()];

        MTLCompileOptions* compileOptions = [[MTLCompileOptions alloc] init];
        if (@available(macOS 13.0, *))
        {
            compileOptions.languageVersion = MTLLanguageVersion3_0;
        }

        NSError* error = nil;
        id<MTLLibrary> library = [device newLibraryWithSource:source
                                                      options:compileOptions
                                                        error:&error];

        if (!library)
        {
            return Result<RHIShaderModuleHandle>::Err(
                std::string("MSL compilation failed: ") + NSErrorToString(error));
        }

        // Get function (SPIRV-Cross uses "main0" as default entry point)
        id<MTLFunction> function = [library newFunctionWithName:@"main0"];
        if (!function)
        {
            return Result<RHIShaderModuleHandle>::Err("Failed to find main0 function in compiled MSL");
        }

        MetalResourceRegistry::ShaderModuleData data;
        data.function = function;
        data.stage = stage;
        data.entryPoint = "main0";

        return Result<RHIShaderModuleHandle>::Ok(MetalResourceRegistry::Get().RegisterShaderModule(data));
    }

    void RHIDevice::DestroyShaderModule(RHIShaderModuleHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterShaderModule(handle);
    }

    // ========================================================================
    // Bindless Texture Support
    // ========================================================================

    uint32_t RHIDevice::GetMaxBindlessTextures() const
    {
        return MetalContext::Get().GetBindlessLimits().maxPerStageDescriptorSampledImages;
    }

    RHIDescriptorSetLayoutHandle RHIDevice::CreateBindlessTextureLayout(uint32_t maxTextures)
    {
        // For Metal, we don't need explicit argument encoders for simple texture arrays
        MetalResourceRegistry::DescriptorSetLayoutData data;
        data.maxTextures = maxTextures;
        return MetalResourceRegistry::Get().RegisterDescriptorSetLayout(data);
    }

    RHIDescriptorSetLayoutHandle RHIDevice::CreateBindlessSamplerTextureLayout(
        RHISamplerHandle immutableSampler, uint32_t maxTextures)
    {
        MetalResourceRegistry::DescriptorSetLayoutData data;
        data.maxTextures = maxTextures;
        return MetalResourceRegistry::Get().RegisterDescriptorSetLayout(data);
    }

    RHIDescriptorSetHandle RHIDevice::AllocateBindlessDescriptorSet(
        RHIDescriptorSetLayoutHandle layout, uint32_t maxTextures)
    {
        MetalResourceRegistry::DescriptorSetData data;
        data.layoutHandle = layout;
        return MetalResourceRegistry::Get().RegisterDescriptorSet(data);
    }

    RHIDescriptorSetHandle RHIDevice::AllocateBindlessSamplerTextureSet(
        RHIDescriptorSetLayoutHandle layout, uint32_t maxTextures)
    {
        return AllocateBindlessDescriptorSet(layout, maxTextures);
    }

    void RHIDevice::UpdateBindlessTexture(RHIDescriptorSetHandle set, uint32_t index,
                                          RHITextureHandle texture, RHISamplerHandle sampler)
    {
        // Metal handles texture binding directly in the encoder
        // This is a no-op - we bind textures directly when rendering
    }

    void RHIDevice::UpdateBindlessSamplerTextureSlot(
        RHIDescriptorSetHandle set, uint32_t index, RHITextureHandle texture)
    {
        // Same as above - Metal binds textures directly
    }

    void* RHIDevice::GetRawDescriptorSet(RHIDescriptorSetHandle handle) const
    {
        auto data = MetalResourceRegistry::Get().GetDescriptorSetData(handle);
        return (__bridge void*)data.argumentBuffer;
    }

    // ========================================================================
    // Descriptor Set Management (stub implementations)
    // ========================================================================

    RHIDescriptorSetLayoutHandle RHIDevice::CreateDescriptorSetLayout(const std::vector<RHIDescriptorBinding>& bindings)
    {
        MetalResourceRegistry::DescriptorSetLayoutData data;
        return MetalResourceRegistry::Get().RegisterDescriptorSetLayout(data);
    }

    void RHIDevice::DestroyDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterDescriptorSetLayout(handle);
    }

    RHIDescriptorSetHandle RHIDevice::AllocateDescriptorSet(RHIDescriptorSetLayoutHandle layout)
    {
        MetalResourceRegistry::DescriptorSetData data;
        data.layoutHandle = layout;
        return MetalResourceRegistry::Get().RegisterDescriptorSet(data);
    }

    void RHIDevice::FreeDescriptorSet(RHIDescriptorSetHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterDescriptorSet(handle);
    }

    void RHIDevice::UpdateDescriptorSet(RHIDescriptorSetHandle set, const std::vector<RHIDescriptorWrite>& writes)
    {
        // Metal uses direct resource binding, not descriptor updates
    }

    // ========================================================================
    // Pipeline Management
    // ========================================================================

    RHIGraphicsPipelineResult RHIDevice::CreateGraphicsPipeline(const RHIGraphicsPipelineSpecification& spec)
    {
        auto result = TryCreateGraphicsPipeline(spec);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateGraphicsPipeline failed: {}", result.Error());
            return RHIGraphicsPipelineResult{NullPipeline, NullPipelineLayout};
        }
        return result.Value();
    }

    Result<RHIGraphicsPipelineResult> RHIDevice::TryCreateGraphicsPipeline(const RHIGraphicsPipelineSpecification& spec)
    {
        id<MTLDevice> device = MetalContext::Get().GetDevice();

        MTLRenderPipelineDescriptor* pipelineDesc = [[MTLRenderPipelineDescriptor alloc] init];

        // Attach shaders
        id<MTLFunction> vertexFunc = nil;
        id<MTLFunction> fragmentFunc = nil;

        for (auto moduleHandle : spec.shaderModules)
        {
            auto moduleData = MetalResourceRegistry::Get().GetShaderModuleData(moduleHandle);
            if (moduleData.stage == ShaderStage::Vertex)
                vertexFunc = moduleData.function;
            else if (moduleData.stage == ShaderStage::Fragment)
                fragmentFunc = moduleData.function;
        }

        if (!vertexFunc)
        {
            return Result<RHIGraphicsPipelineResult>::Err("No vertex shader provided");
        }

        pipelineDesc.vertexFunction = vertexFunc;
        pipelineDesc.fragmentFunction = fragmentFunc;

        // Vertex descriptor
        if (!spec.vertexBindings.empty())
        {
            MTLVertexDescriptor* vertexDesc = [[MTLVertexDescriptor alloc] init];

            for (const auto& binding : spec.vertexBindings)
            {
                vertexDesc.layouts[binding.binding].stride = binding.stride;
                vertexDesc.layouts[binding.binding].stepFunction =
                    (binding.inputRate == VertexInputRate::Instance)
                    ? MTLVertexStepFunctionPerInstance
                    : MTLVertexStepFunctionPerVertex;
            }

            for (const auto& attr : spec.vertexAttributes)
            {
                vertexDesc.attributes[attr.location].format = ToMetalVertexFormat(attr.format);
                vertexDesc.attributes[attr.location].offset = attr.offset;
                vertexDesc.attributes[attr.location].bufferIndex = attr.binding;
            }

            pipelineDesc.vertexDescriptor = vertexDesc;
        }

        // Render pass color format
        auto rpData = MetalResourceRegistry::Get().GetRenderPassData(spec.renderPass);
        pipelineDesc.colorAttachments[0].pixelFormat = ToMetal(rpData.colorFormat);

        // Blending
        if (!spec.colorBlendStates.empty())
        {
            const auto& blend = spec.colorBlendStates[0];
            pipelineDesc.colorAttachments[0].blendingEnabled = blend.blendEnable;
            pipelineDesc.colorAttachments[0].sourceRGBBlendFactor = ToMetal(blend.srcColorBlendFactor);
            pipelineDesc.colorAttachments[0].destinationRGBBlendFactor = ToMetal(blend.dstColorBlendFactor);
            pipelineDesc.colorAttachments[0].rgbBlendOperation = ToMetal(blend.colorBlendOp);
            pipelineDesc.colorAttachments[0].sourceAlphaBlendFactor = ToMetal(blend.srcAlphaBlendFactor);
            pipelineDesc.colorAttachments[0].destinationAlphaBlendFactor = ToMetal(blend.dstAlphaBlendFactor);
            pipelineDesc.colorAttachments[0].alphaBlendOperation = ToMetal(blend.alphaBlendOp);
            pipelineDesc.colorAttachments[0].writeMask = ToMetal(blend.colorWriteMask);
        }
        else
        {
            pipelineDesc.colorAttachments[0].writeMask = MTLColorWriteMaskAll;
        }

        // Depth format
        if (rpData.depthFormat != TextureFormat::Undefined)
        {
            pipelineDesc.depthAttachmentPixelFormat = ToMetal(rpData.depthFormat);
        }

        // Debug name
        if (!spec.debugName.empty())
        {
            pipelineDesc.label = [NSString stringWithUTF8String:spec.debugName.c_str()];
        }

        // Create pipeline
        NSError* error = nil;
        id<MTLRenderPipelineState> pipeline = [device newRenderPipelineStateWithDescriptor:pipelineDesc
                                                                                     error:&error];

        if (!pipeline)
        {
            return Result<RHIGraphicsPipelineResult>::Err(
                std::string("Failed to create pipeline: ") + NSErrorToString(error));
        }

        // Create depth stencil state if needed
        id<MTLDepthStencilState> depthState = nil;
        if (spec.depthTestEnable || spec.depthWriteEnable)
        {
            MTLDepthStencilDescriptor* depthDesc = [[MTLDepthStencilDescriptor alloc] init];
            depthDesc.depthCompareFunction = spec.depthTestEnable ? ToMetal(spec.depthCompareOp) : MTLCompareFunctionAlways;
            depthDesc.depthWriteEnabled = spec.depthWriteEnable;
            depthState = [device newDepthStencilStateWithDescriptor:depthDesc];
        }

        // Register pipeline
        MetalResourceRegistry::PipelineData pipelineData;
        pipelineData.pipeline = pipeline;
        pipelineData.depthStencilState = depthState;
        pipelineData.cullMode = ToMetal(spec.cullMode);
        pipelineData.frontFace = ToMetal(spec.frontFace);
        pipelineData.fillMode = ToMetal(spec.polygonMode);

        RHIPipelineHandle pipelineHandle = MetalResourceRegistry::Get().RegisterPipeline(pipelineData);

        // Register pipeline layout
        MetalResourceRegistry::PipelineLayoutData layoutData;
        layoutData.setLayouts = spec.descriptorSetLayouts;
        RHIPipelineLayoutHandle layoutHandle = MetalResourceRegistry::Get().RegisterPipelineLayout(layoutData);

        return Result<RHIGraphicsPipelineResult>::Ok(RHIGraphicsPipelineResult{pipelineHandle, layoutHandle});
    }

    void RHIDevice::DestroyPipeline(RHIPipelineHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterPipeline(handle);
    }

    void RHIDevice::DestroyPipelineLayout(RHIPipelineLayoutHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterPipelineLayout(handle);
    }

    RHIPipelineLayoutHandle RHIDevice::GetPipelineLayout(RHIPipelineHandle pipeline) const
    {
        // Metal doesn't separate pipeline and layout, return a placeholder
        return NullPipelineLayout;
    }

    // ========================================================================
    // Render Pass Management
    // ========================================================================

    RHIRenderPassHandle RHIDevice::CreateRenderPass(const RHIRenderPassSpecification& spec)
    {
        MetalResourceRegistry::RenderPassData data;
        if (!spec.colorAttachments.empty())
        {
            data.colorFormat = spec.colorAttachments[0].format;
            data.colorLoadOp = spec.colorAttachments[0].loadOp;
            data.colorStoreOp = spec.colorAttachments[0].storeOp;
        }
        if (spec.depthStencilAttachment.has_value())
        {
            data.depthFormat = spec.depthStencilAttachment->format;
            data.depthLoadOp = spec.depthStencilAttachment->loadOp;
            data.depthStoreOp = spec.depthStencilAttachment->storeOp;
        }
        return MetalResourceRegistry::Get().RegisterRenderPass(data);
    }

    void RHIDevice::DestroyRenderPass(RHIRenderPassHandle handle)
    {
        MetalResourceRegistry::Get().UnregisterRenderPass(handle);
    }

    // ========================================================================
    // Framebuffer Management
    // ========================================================================

    RHIFramebufferHandle RHIDevice::CreateFramebuffer(const RHIFramebufferSpecification& spec)
    {
        // Metal doesn't have explicit framebuffers - render pass descriptors are created per-draw
        return RHIFramebufferHandle{1}; // Placeholder
    }

    void RHIDevice::DestroyFramebuffer(RHIFramebufferHandle handle)
    {
        // No-op for Metal
    }

    // ========================================================================
    // ImGui Integration
    // ========================================================================

    void* RHIDevice::RegisterImGuiTexture(RHITextureHandle texture, RHISamplerHandle sampler)
    {
        auto texData = MetalResourceRegistry::Get().GetTextureData(texture);
        return (__bridge void*)texData.texture;
    }

    void RHIDevice::UnregisterImGuiTexture(void* imguiHandle)
    {
        // No-op for Metal
    }

    // ============================================================================
    // RHICmd Implementation (Command Recording)
    // ============================================================================

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, float x, float y, float width, float height,
                             float minDepth, float maxDepth)
    {
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (!encoder) return;

        MTLViewport viewport;
        viewport.originX = x;
        viewport.originY = y;
        viewport.width = width;
        viewport.height = height;
        viewport.znear = minDepth;
        viewport.zfar = maxDepth;
        [encoder setViewport:viewport];
    }

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, float width, float height)
    {
        SetViewport(cmd, 0, 0, width, height, 0.0f, 1.0f);
    }

    void RHICmd::SetViewport(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        SetViewport(cmd, 0, 0, static_cast<float>(width), static_cast<float>(height), 0.0f, 1.0f);
    }

    void RHICmd::SetScissor(RHICommandBufferHandle cmd, int32_t x, int32_t y, uint32_t width, uint32_t height)
    {
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (!encoder) return;

        MTLScissorRect scissor;
        scissor.x = static_cast<NSUInteger>(std::max(0, x));
        scissor.y = static_cast<NSUInteger>(std::max(0, y));
        scissor.width = width;
        scissor.height = height;
        [encoder setScissorRect:scissor];
    }

    void RHICmd::SetScissor(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        SetScissor(cmd, 0, 0, width, height);
    }

    void RHICmd::BindPipeline(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline)
    {
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (!encoder) return;

        auto data = MetalResourceRegistry::Get().GetPipelineData(pipeline);
        [encoder setRenderPipelineState:data.pipeline];
        [encoder setCullMode:data.cullMode];
        [encoder setFrontFacingWinding:data.frontFace];
        [encoder setTriangleFillMode:data.fillMode];

        if (data.depthStencilState)
        {
            [encoder setDepthStencilState:data.depthStencilState];
        }
    }

    void RHICmd::BindVertexBuffer(RHICommandBufferHandle cmd, RHIBufferHandle buffer, uint32_t binding)
    {
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (!encoder) return;

        auto data = MetalResourceRegistry::Get().GetBufferData(buffer);
        [encoder setVertexBuffer:data.buffer offset:0 atIndex:binding];
    }

    void RHICmd::BindIndexBuffer(RHICommandBufferHandle cmd, RHIBufferHandle buffer, IndexType indexType)
    {
        // Metal binds index buffer at draw time, not separately
        // Store for later use - this is handled in DrawIndexed
    }

    void RHICmd::BindDescriptorSet(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                                    RHIDescriptorSetHandle set, uint32_t setIndex)
    {
        // Metal uses direct resource binding - handle in specific bind calls
    }

    void RHICmd::BindDescriptorSet(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                    RHIDescriptorSetHandle set, uint32_t setIndex)
    {
        // Metal uses direct resource binding
    }

    void RHICmd::BindDescriptorSetRaw(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                       void* descriptorSet, uint32_t setIndex)
    {
        // For bindless textures in Metal, we'd set the argument buffer here
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (!encoder) return;

        id<MTLBuffer> argBuffer = (__bridge id<MTLBuffer>)descriptorSet;
        if (argBuffer)
        {
            [encoder setFragmentBuffer:argBuffer offset:0 atIndex:setIndex];
        }
    }

    void RHICmd::Draw(RHICommandBufferHandle cmd, uint32_t vertexCount, uint32_t instanceCount,
                      uint32_t firstVertex, uint32_t firstInstance)
    {
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (!encoder) return;

        [encoder drawPrimitives:MTLPrimitiveTypeTriangle
                    vertexStart:firstVertex
                    vertexCount:vertexCount
                  instanceCount:instanceCount
                   baseInstance:firstInstance];
    }

    void RHICmd::DrawIndexed(RHICommandBufferHandle cmd, uint32_t indexCount, uint32_t instanceCount,
                             uint32_t firstIndex, int32_t vertexOffset, uint32_t firstInstance)
    {
        // Note: This requires the index buffer to be bound separately
        // For now, we'll need to track the bound index buffer
        GG_CORE_WARN("RHICmd::DrawIndexed requires tracked index buffer - implement tracking");
    }

    void RHICmd::BeginRenderPass(RHICommandBufferHandle cmd, RHIRenderPassHandle renderPass,
                                  RHIFramebufferHandle framebuffer, uint32_t width, uint32_t height,
                                  float clearR, float clearG, float clearB, float clearA)
    {
        // Metal begins render passes when creating the encoder
        // This is handled differently than Vulkan
    }

    void RHICmd::EndRenderPass(RHICommandBufferHandle cmd)
    {
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (encoder)
        {
            [encoder endEncoding];
        }
    }

    void RHICmd::CopyBuffer(RHICommandBufferHandle cmd, RHIBufferHandle src, RHIBufferHandle dst,
                            uint64_t srcOffset, uint64_t dstOffset, uint64_t size)
    {
        auto cmdBuffer = MetalResourceRegistry::Get().GetCommandBuffer(cmd);
        if (!cmdBuffer) return;

        auto srcData = MetalResourceRegistry::Get().GetBufferData(src);
        auto dstData = MetalResourceRegistry::Get().GetBufferData(dst);

        id<MTLBlitCommandEncoder> blit = [cmdBuffer blitCommandEncoder];
        [blit copyFromBuffer:srcData.buffer sourceOffset:srcOffset
                    toBuffer:dstData.buffer destinationOffset:dstOffset
                        size:size];
        [blit endEncoding];
    }

    void RHICmd::CopyBufferToTexture(RHICommandBufferHandle cmd, RHIBufferHandle buffer,
                                      RHITextureHandle texture, uint32_t width, uint32_t height)
    {
        auto cmdBuffer = MetalResourceRegistry::Get().GetCommandBuffer(cmd);
        if (!cmdBuffer) return;

        auto bufData = MetalResourceRegistry::Get().GetBufferData(buffer);
        auto texData = MetalResourceRegistry::Get().GetTextureData(texture);

        id<MTLBlitCommandEncoder> blit = [cmdBuffer blitCommandEncoder];
        [blit copyFromBuffer:bufData.buffer
                sourceOffset:0
           sourceBytesPerRow:width * 4  // Assuming RGBA8
         sourceBytesPerImage:width * height * 4
                  sourceSize:MTLSizeMake(width, height, 1)
                   toTexture:texData.texture
            destinationSlice:0
            destinationLevel:0
           destinationOrigin:MTLOriginMake(0, 0, 0)];
        [blit endEncoding];
    }

    void RHICmd::TransitionImageLayout(RHICommandBufferHandle cmd, RHITextureHandle texture,
                                        ImageLayout oldLayout, ImageLayout newLayout)
    {
        // Metal handles resource synchronization automatically for most cases
        // Explicit barriers only needed for specific scenarios
    }

    void RHICmd::PipelineBarrier(RHICommandBufferHandle cmd, const RHIPipelineBarrier& barrier)
    {
        // Metal uses automatic memory management for most synchronization
    }

    // Push constants via buffer at index 0
    void RHICmd::PushConstants(RHICommandBufferHandle cmd, RHIPipelineHandle pipeline,
                                ShaderStage stages, uint32_t offset, uint32_t size, const void* data)
    {
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (!encoder) return;

        if (HasFlag(stages, ShaderStage::Vertex))
        {
            [encoder setVertexBytes:data length:size atIndex:0];
        }
        if (HasFlag(stages, ShaderStage::Fragment))
        {
            [encoder setFragmentBytes:data length:size atIndex:0];
        }
    }

    void RHICmd::PushConstants(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout,
                                ShaderStage stages, uint32_t offset, uint32_t size, const void* data)
    {
        auto encoder = MetalContext::Get().GetCurrentRenderEncoder();
        if (!encoder) return;

        if (HasFlag(stages, ShaderStage::Vertex))
        {
            [encoder setVertexBytes:data length:size atIndex:0];
        }
        if (HasFlag(stages, ShaderStage::Fragment))
        {
            [encoder setFragmentBytes:data length:size atIndex:0];
        }
    }

}
