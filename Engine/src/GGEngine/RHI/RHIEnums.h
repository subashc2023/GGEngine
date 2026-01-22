#pragma once

#include "GGEngine/Core/Core.h"
#include <cstdint>

namespace GGEngine {

    // ============================================================================
    // Primitive Topology
    // ============================================================================
    enum class PrimitiveTopology : uint8_t
    {
        PointList,
        LineList,
        LineStrip,
        TriangleList,
        TriangleStrip,
        TriangleFan
    };

    // ============================================================================
    // Polygon Mode (Fill Mode)
    // ============================================================================
    enum class PolygonMode : uint8_t
    {
        Fill,
        Line,
        Point
    };

    // ============================================================================
    // Cull Mode
    // ============================================================================
    enum class CullMode : uint8_t
    {
        None = 0,
        Front = 1,
        Back = 2,
        FrontAndBack = 3
    };

    // ============================================================================
    // Front Face Winding Order
    // ============================================================================
    enum class FrontFace : uint8_t
    {
        CounterClockwise,
        Clockwise
    };

    // ============================================================================
    // Comparison Operations (depth, stencil)
    // ============================================================================
    enum class CompareOp : uint8_t
    {
        Never,
        Less,
        Equal,
        LessOrEqual,
        Greater,
        NotEqual,
        GreaterOrEqual,
        Always
    };

    // ============================================================================
    // Sample Count (MSAA)
    // ============================================================================
    enum class SampleCount : uint8_t
    {
        Count1 = 1,
        Count2 = 2,
        Count4 = 4,
        Count8 = 8,
        Count16 = 16,
        Count32 = 32,
        Count64 = 64
    };

    // ============================================================================
    // Shader Stage Flags (bitmask)
    // ============================================================================
    enum class ShaderStage : uint32_t
    {
        None = 0,
        Vertex = 1 << 0,
        TessellationControl = 1 << 1,
        TessellationEvaluation = 1 << 2,
        Geometry = 1 << 3,
        Fragment = 1 << 4,
        Compute = 1 << 5,

        // Common combinations
        AllGraphics = Vertex | TessellationControl | TessellationEvaluation | Geometry | Fragment,
        All = AllGraphics | Compute,
        VertexFragment = Vertex | Fragment
    };

    inline ShaderStage operator|(ShaderStage a, ShaderStage b)
    {
        return static_cast<ShaderStage>(static_cast<uint32_t>(a) | static_cast<uint32_t>(b));
    }

    inline ShaderStage operator&(ShaderStage a, ShaderStage b)
    {
        return static_cast<ShaderStage>(static_cast<uint32_t>(a) & static_cast<uint32_t>(b));
    }

    inline ShaderStage& operator|=(ShaderStage& a, ShaderStage b)
    {
        a = a | b;
        return a;
    }

    inline bool HasFlag(ShaderStage flags, ShaderStage flag)
    {
        return (static_cast<uint32_t>(flags) & static_cast<uint32_t>(flag)) != 0;
    }

    // ============================================================================
    // Descriptor Type
    // ============================================================================
    enum class DescriptorType : uint8_t
    {
        Sampler,
        CombinedImageSampler,
        SampledImage,
        StorageImage,
        UniformTexelBuffer,
        StorageTexelBuffer,
        UniformBuffer,
        StorageBuffer,
        UniformBufferDynamic,
        StorageBufferDynamic,
        InputAttachment
    };

    // ============================================================================
    // Texture/Image Format
    // ============================================================================
    enum class TextureFormat : uint8_t
    {
        Undefined = 0,

        // 8-bit per channel
        R8_UNORM,
        R8_SNORM,
        R8_UINT,
        R8_SINT,
        R8G8_UNORM,
        R8G8_SNORM,
        R8G8_UINT,
        R8G8_SINT,
        R8G8B8_UNORM,
        R8G8B8_SRGB,
        R8G8B8A8_UNORM,
        R8G8B8A8_SNORM,
        R8G8B8A8_UINT,
        R8G8B8A8_SINT,
        R8G8B8A8_SRGB,
        B8G8R8A8_UNORM,
        B8G8R8A8_SRGB,

        // 16-bit per channel
        R16_UNORM,
        R16_SNORM,
        R16_UINT,
        R16_SINT,
        R16_SFLOAT,
        R16G16_UNORM,
        R16G16_SNORM,
        R16G16_UINT,
        R16G16_SINT,
        R16G16_SFLOAT,
        R16G16B16A16_UNORM,
        R16G16B16A16_SNORM,
        R16G16B16A16_UINT,
        R16G16B16A16_SINT,
        R16G16B16A16_SFLOAT,

        // 32-bit per channel
        R32_UINT,
        R32_SINT,
        R32_SFLOAT,
        R32G32_UINT,
        R32G32_SINT,
        R32G32_SFLOAT,
        R32G32B32_UINT,
        R32G32B32_SINT,
        R32G32B32_SFLOAT,
        R32G32B32A32_UINT,
        R32G32B32A32_SINT,
        R32G32B32A32_SFLOAT,

        // Depth/Stencil
        D16_UNORM,
        D32_SFLOAT,
        D24_UNORM_S8_UINT,
        D32_SFLOAT_S8_UINT,
        S8_UINT,

        // Compressed formats (common)
        BC1_RGB_UNORM,
        BC1_RGB_SRGB,
        BC1_RGBA_UNORM,
        BC1_RGBA_SRGB,
        BC2_UNORM,
        BC2_SRGB,
        BC3_UNORM,
        BC3_SRGB,
        BC4_UNORM,
        BC4_SNORM,
        BC5_UNORM,
        BC5_SNORM,
        BC6H_UFLOAT,
        BC6H_SFLOAT,
        BC7_UNORM,
        BC7_SRGB
    };

    // Helper to check if format is depth/stencil
    inline bool IsDepthFormat(TextureFormat format)
    {
        switch (format)
        {
            case TextureFormat::D16_UNORM:
            case TextureFormat::D32_SFLOAT:
            case TextureFormat::D24_UNORM_S8_UINT:
            case TextureFormat::D32_SFLOAT_S8_UINT:
                return true;
            default:
                return false;
        }
    }

    inline bool HasStencil(TextureFormat format)
    {
        switch (format)
        {
            case TextureFormat::D24_UNORM_S8_UINT:
            case TextureFormat::D32_SFLOAT_S8_UINT:
            case TextureFormat::S8_UINT:
                return true;
            default:
                return false;
        }
    }

    // ============================================================================
    // Texture Filter Mode
    // ============================================================================
    enum class Filter : uint8_t
    {
        Nearest,
        Linear
    };

    // ============================================================================
    // Texture Mipmap Mode
    // ============================================================================
    enum class MipmapMode : uint8_t
    {
        Nearest,
        Linear
    };

    // ============================================================================
    // Texture Address Mode (Wrap Mode)
    // ============================================================================
    enum class AddressMode : uint8_t
    {
        Repeat,
        MirroredRepeat,
        ClampToEdge,
        ClampToBorder,
        MirrorClampToEdge
    };

    // ============================================================================
    // Border Color (for ClampToBorder address mode)
    // ============================================================================
    enum class BorderColor : uint8_t
    {
        FloatTransparentBlack,
        IntTransparentBlack,
        FloatOpaqueBlack,
        IntOpaqueBlack,
        FloatOpaqueWhite,
        IntOpaqueWhite
    };

    // ============================================================================
    // Index Type
    // ============================================================================
    enum class IndexType : uint8_t
    {
        UInt16,
        UInt32
    };

    // ============================================================================
    // Buffer Usage (matches existing BufferUsage but explicit here for clarity)
    // ============================================================================
    enum class BufferUsage : uint8_t
    {
        Vertex,
        Index,
        Uniform,
        Storage,
        Staging,
        Indirect
    };

    // ============================================================================
    // Blend Factor
    // ============================================================================
    enum class BlendFactor : uint8_t
    {
        Zero,
        One,
        SrcColor,
        OneMinusSrcColor,
        DstColor,
        OneMinusDstColor,
        SrcAlpha,
        OneMinusSrcAlpha,
        DstAlpha,
        OneMinusDstAlpha,
        ConstantColor,
        OneMinusConstantColor,
        ConstantAlpha,
        OneMinusConstantAlpha,
        SrcAlphaSaturate
    };

    // ============================================================================
    // Blend Operation
    // ============================================================================
    enum class BlendOp : uint8_t
    {
        Add,
        Subtract,
        ReverseSubtract,
        Min,
        Max
    };

    // ============================================================================
    // Color Component Flags (bitmask)
    // ============================================================================
    enum class ColorComponent : uint8_t
    {
        None = 0,
        R = 1 << 0,
        G = 1 << 1,
        B = 1 << 2,
        A = 1 << 3,
        All = R | G | B | A
    };

    inline ColorComponent operator|(ColorComponent a, ColorComponent b)
    {
        return static_cast<ColorComponent>(static_cast<uint8_t>(a) | static_cast<uint8_t>(b));
    }

    inline ColorComponent operator&(ColorComponent a, ColorComponent b)
    {
        return static_cast<ColorComponent>(static_cast<uint8_t>(a) & static_cast<uint8_t>(b));
    }

    // ============================================================================
    // Attachment Load Operation
    // ============================================================================
    enum class LoadOp : uint8_t
    {
        Load,
        Clear,
        DontCare
    };

    // ============================================================================
    // Attachment Store Operation
    // ============================================================================
    enum class StoreOp : uint8_t
    {
        Store,
        DontCare
    };

    // ============================================================================
    // Image Layout
    // ============================================================================
    enum class ImageLayout : uint8_t
    {
        Undefined,
        General,
        ColorAttachment,
        DepthStencilAttachment,
        DepthStencilReadOnly,
        ShaderReadOnly,
        TransferSrc,
        TransferDst,
        Present
    };

}
