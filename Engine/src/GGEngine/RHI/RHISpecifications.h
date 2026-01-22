#pragma once

#include "RHITypes.h"
#include "RHIEnums.h"
#include "GGEngine/Core/Core.h"

#include <string>
#include <vector>
#include <optional>
#include <variant>

namespace GGEngine {

    // ============================================================================
    // Texture Usage Flags (bitmask)
    // ============================================================================
    enum class TextureUsage : uint32_t
    {
        None = 0,
        Sampled = 1 << 0,           // Can be sampled in shaders
        Storage = 1 << 1,           // Can be used as storage image
        ColorAttachment = 1 << 2,   // Can be used as color attachment
        DepthStencilAttachment = 1 << 3,  // Can be used as depth/stencil attachment
        TransferSrc = 1 << 4,       // Can be source of transfer operations
        TransferDst = 1 << 5,       // Can be destination of transfer operations
        InputAttachment = 1 << 6,   // Can be used as input attachment
    };

    inline TextureUsage operator|(TextureUsage a, TextureUsage b)
    {
        return static_cast<TextureUsage>(static_cast<uint32_t>(a) | static_cast<uint32_t>(b));
    }

    inline TextureUsage operator&(TextureUsage a, TextureUsage b)
    {
        return static_cast<TextureUsage>(static_cast<uint32_t>(a) & static_cast<uint32_t>(b));
    }

    inline bool HasFlag(TextureUsage flags, TextureUsage flag)
    {
        return (static_cast<uint32_t>(flags) & static_cast<uint32_t>(flag)) != 0;
    }

    // ============================================================================
    // Push Constant Range (backend-agnostic)
    // ============================================================================
    struct GG_API RHIPushConstantRange
    {
        ShaderStage stages = ShaderStage::AllGraphics;
        uint32_t offset = 0;
        uint32_t size = 0;
    };

    // ============================================================================
    // Buffer Specification (for RHIDevice::CreateBuffer)
    // Note: Mirrors BufferSpecification in Buffer.h for RHI layer usage
    // ============================================================================
    struct GG_API RHIBufferSpecification
    {
        uint64_t size = 0;
        BufferUsage usage = BufferUsage::Vertex;
        bool cpuVisible = false;
        std::string debugName;
    };

    // ============================================================================
    // Texture Specification (for RHIDevice::CreateTexture)
    // ============================================================================
    struct GG_API RHITextureSpecification
    {
        uint32_t width = 1;
        uint32_t height = 1;
        uint32_t depth = 1;
        uint32_t mipLevels = 1;
        uint32_t arrayLayers = 1;
        TextureFormat format = TextureFormat::R8G8B8A8_UNORM;
        SampleCount samples = SampleCount::Count1;
        TextureUsage usage = TextureUsage::Sampled | TextureUsage::TransferDst;
        ImageLayout initialLayout = ImageLayout::Undefined;
        std::string debugName;
    };

    // ============================================================================
    // Render Pass Attachment Description
    // ============================================================================
    struct GG_API RHIAttachmentDescription
    {
        TextureFormat format = TextureFormat::B8G8R8A8_UNORM;
        SampleCount samples = SampleCount::Count1;
        LoadOp loadOp = LoadOp::Clear;
        StoreOp storeOp = StoreOp::Store;
        LoadOp stencilLoadOp = LoadOp::DontCare;
        StoreOp stencilStoreOp = StoreOp::DontCare;
        ImageLayout initialLayout = ImageLayout::Undefined;
        ImageLayout finalLayout = ImageLayout::Present;
    };

    // ============================================================================
    // Render Pass Specification (for RHIDevice::CreateRenderPass)
    // ============================================================================
    struct GG_API RHIRenderPassSpecification
    {
        std::vector<RHIAttachmentDescription> colorAttachments;
        std::optional<RHIAttachmentDescription> depthStencilAttachment;
        std::string debugName;
    };

    // ============================================================================
    // Framebuffer Specification (for RHIDevice::CreateFramebuffer)
    // ============================================================================
    struct GG_API RHIFramebufferSpecification
    {
        RHIRenderPassHandle renderPass;
        std::vector<RHITextureHandle> attachments;
        uint32_t width = 0;
        uint32_t height = 0;
        uint32_t layers = 1;
        std::string debugName;
    };

    // ============================================================================
    // Blend State Description
    // ============================================================================
    struct GG_API RHIBlendState
    {
        bool enable = false;
        BlendFactor srcColorFactor = BlendFactor::One;
        BlendFactor dstColorFactor = BlendFactor::Zero;
        BlendOp colorOp = BlendOp::Add;
        BlendFactor srcAlphaFactor = BlendFactor::One;
        BlendFactor dstAlphaFactor = BlendFactor::Zero;
        BlendOp alphaOp = BlendOp::Add;
        ColorComponent colorWriteMask = ColorComponent::All;

        // Preset constructors
        static RHIBlendState Opaque()
        {
            return RHIBlendState{};
        }

        static RHIBlendState Alpha()
        {
            RHIBlendState state;
            state.enable = true;
            state.srcColorFactor = BlendFactor::SrcAlpha;
            state.dstColorFactor = BlendFactor::OneMinusSrcAlpha;
            state.srcAlphaFactor = BlendFactor::One;
            state.dstAlphaFactor = BlendFactor::OneMinusSrcAlpha;
            return state;
        }

        static RHIBlendState Additive()
        {
            RHIBlendState state;
            state.enable = true;
            state.srcColorFactor = BlendFactor::SrcAlpha;
            state.dstColorFactor = BlendFactor::One;
            state.srcAlphaFactor = BlendFactor::One;
            state.dstAlphaFactor = BlendFactor::One;
            return state;
        }
    };

    // ============================================================================
    // Graphics Pipeline Specification (for RHIDevice::CreateGraphicsPipeline)
    // ============================================================================
    struct GG_API RHIGraphicsPipelineSpecification
    {
        // Shader modules (vertex, fragment, etc.)
        std::vector<RHIShaderModuleHandle> shaderModules;

        // Render target
        RHIRenderPassHandle renderPass;
        uint32_t subpass = 0;

        // Vertex input (optional - can be empty for full-screen triangle shaders)
        std::vector<RHIVertexBindingDescription> vertexBindings;
        std::vector<RHIVertexAttributeDescription> vertexAttributes;

        // Input assembly
        PrimitiveTopology topology = PrimitiveTopology::TriangleList;

        // Rasterization
        PolygonMode polygonMode = PolygonMode::Fill;
        CullMode cullMode = CullMode::None;
        FrontFace frontFace = FrontFace::Clockwise;
        float lineWidth = 1.0f;

        // Multisampling
        SampleCount samples = SampleCount::Count1;

        // Depth/Stencil
        bool depthTestEnable = false;
        bool depthWriteEnable = false;
        CompareOp depthCompareOp = CompareOp::Less;

        // Color blending (per attachment - typically just one)
        std::vector<RHIBlendState> colorBlendStates;

        // Descriptor set layouts
        std::vector<RHIDescriptorSetLayoutHandle> descriptorSetLayouts;

        // Push constants
        std::vector<RHIPushConstantRange> pushConstantRanges;

        // Debug
        std::string debugName;
    };

    // ============================================================================
    // Descriptor Write - Buffer Info
    // ============================================================================
    struct GG_API RHIDescriptorBufferInfo
    {
        RHIBufferHandle buffer;
        uint64_t offset = 0;
        uint64_t range = 0;  // 0 = whole buffer
    };

    // ============================================================================
    // Descriptor Write - Image Info
    // ============================================================================
    struct GG_API RHIDescriptorImageInfo
    {
        RHISamplerHandle sampler;
        RHITextureHandle texture;
        ImageLayout layout = ImageLayout::ShaderReadOnly;
    };

    // ============================================================================
    // Descriptor Write Operation (for RHIDevice::UpdateDescriptorSet)
    // ============================================================================
    struct GG_API RHIDescriptorWrite
    {
        uint32_t binding = 0;
        uint32_t arrayElement = 0;
        DescriptorType type = DescriptorType::UniformBuffer;
        std::variant<RHIDescriptorBufferInfo, RHIDescriptorImageInfo> resource;

        // Convenience constructors
        static RHIDescriptorWrite UniformBuffer(uint32_t binding, RHIBufferHandle buffer,
                                                 uint64_t offset = 0, uint64_t range = 0)
        {
            RHIDescriptorWrite write;
            write.binding = binding;
            write.type = DescriptorType::UniformBuffer;
            write.resource = RHIDescriptorBufferInfo{ buffer, offset, range };
            return write;
        }

        static RHIDescriptorWrite CombinedImageSampler(uint32_t binding, RHITextureHandle texture,
                                                        RHISamplerHandle sampler,
                                                        ImageLayout layout = ImageLayout::ShaderReadOnly)
        {
            RHIDescriptorWrite write;
            write.binding = binding;
            write.type = DescriptorType::CombinedImageSampler;
            write.resource = RHIDescriptorImageInfo{ sampler, texture, layout };
            return write;
        }

        static RHIDescriptorWrite StorageBuffer(uint32_t binding, RHIBufferHandle buffer,
                                                 uint64_t offset = 0, uint64_t range = 0)
        {
            RHIDescriptorWrite write;
            write.binding = binding;
            write.type = DescriptorType::StorageBuffer;
            write.resource = RHIDescriptorBufferInfo{ buffer, offset, range };
            return write;
        }
    };

    // ============================================================================
    // Buffer-to-Image Copy Region
    // ============================================================================
    struct GG_API RHIBufferImageCopy
    {
        uint64_t bufferOffset = 0;
        uint32_t bufferRowLength = 0;    // 0 = tightly packed
        uint32_t bufferImageHeight = 0;  // 0 = tightly packed

        uint32_t imageOffsetX = 0;
        uint32_t imageOffsetY = 0;
        uint32_t imageOffsetZ = 0;
        uint32_t imageWidth = 0;
        uint32_t imageHeight = 0;
        uint32_t imageDepth = 1;

        uint32_t mipLevel = 0;
        uint32_t arrayLayer = 0;
        uint32_t layerCount = 1;
    };

    // ============================================================================
    // Graphics Pipeline Creation Result
    // ============================================================================
    struct GG_API RHIGraphicsPipelineResult
    {
        RHIPipelineHandle pipeline;
        RHIPipelineLayoutHandle layout;

        bool IsValid() const { return pipeline.IsValid() && layout.IsValid(); }
    };

    // ============================================================================
    // Pipeline Barrier (for RHICmd::PipelineBarrier)
    // ============================================================================
    struct GG_API RHIImageBarrier
    {
        RHITextureHandle texture;
        ImageLayout oldLayout = ImageLayout::Undefined;
        ImageLayout newLayout = ImageLayout::ShaderReadOnly;
        uint32_t baseMipLevel = 0;
        uint32_t mipCount = 1;
        uint32_t baseArrayLayer = 0;
        uint32_t layerCount = 1;
    };

    struct GG_API RHIPipelineBarrier
    {
        std::vector<RHIImageBarrier> imageBarriers;
        // Buffer barriers can be added here if needed
    };

}
