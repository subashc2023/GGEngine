#pragma once

#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"

#ifdef __OBJC__
#import <Metal/Metal.h>
#endif

#include <unordered_map>
#include <mutex>
#include <string>
#include <vector>

namespace GGEngine {

    // ============================================================================
    // Metal Type Conversions
    // ============================================================================
    // Convert RHI enums to Metal equivalents

#ifdef __OBJC__
    GG_API MTLPrimitiveType ToMetal(PrimitiveTopology topology);
    GG_API MTLTriangleFillMode ToMetal(PolygonMode mode);
    GG_API MTLCullMode ToMetal(CullMode mode);
    GG_API MTLWinding ToMetal(FrontFace face);
    GG_API MTLCompareFunction ToMetal(CompareOp op);
    GG_API NSUInteger ToMetalSampleCount(SampleCount count);
    GG_API MTLPixelFormat ToMetal(TextureFormat format);
    GG_API MTLSamplerMinMagFilter ToMetal(Filter filter);
    GG_API MTLSamplerMipFilter ToMetalMipFilter(MipmapMode mode);
    GG_API MTLSamplerAddressMode ToMetal(AddressMode mode);
    GG_API MTLIndexType ToMetal(IndexType type);
    GG_API MTLBlendFactor ToMetal(BlendFactor factor);
    GG_API MTLBlendOperation ToMetal(BlendOp op);
    GG_API MTLColorWriteMask ToMetal(ColorComponent components);
    GG_API MTLLoadAction ToMetal(LoadOp op);
    GG_API MTLStoreAction ToMetal(StoreOp op);
    GG_API MTLVertexFormat ToMetalVertexFormat(TextureFormat format);

    // Reverse conversions (Metal to RHI)
    GG_API TextureFormat FromMetal(MTLPixelFormat format);
#endif

    // ============================================================================
    // Metal Resource Registry
    // ============================================================================
    // Maps opaque RHI handles to actual Metal objects.
    // Thread-safe for registration/unregistration.

    class GG_API MetalResourceRegistry
    {
    public:
        static MetalResourceRegistry& Get();

        // ========================================================================
        // Pipeline Management
        // ========================================================================
#ifdef __OBJC__
        struct PipelineData
        {
            id<MTLRenderPipelineState> pipeline = nil;
            id<MTLDepthStencilState> depthStencilState = nil;
            MTLCullMode cullMode = MTLCullModeNone;
            MTLWinding frontFace = MTLWindingClockwise;
            MTLTriangleFillMode fillMode = MTLTriangleFillModeFill;
        };
#else
        struct PipelineData
        {
            void* pipeline = nullptr;
            void* depthStencilState = nullptr;
            int cullMode = 0;
            int frontFace = 0;
            int fillMode = 0;
        };
#endif

        RHIPipelineHandle RegisterPipeline(const PipelineData& data);
        void UnregisterPipeline(RHIPipelineHandle handle);
        PipelineData GetPipelineData(RHIPipelineHandle handle) const;

        // ========================================================================
        // Pipeline Layout Management (Metal doesn't have explicit layouts,
        // but we track descriptor set layout associations)
        // ========================================================================
        struct PipelineLayoutData
        {
            std::vector<RHIDescriptorSetLayoutHandle> setLayouts;
            uint32_t pushConstantSize = 0;
        };

        RHIPipelineLayoutHandle RegisterPipelineLayout(const PipelineLayoutData& data);
        void UnregisterPipelineLayout(RHIPipelineLayoutHandle handle);
        PipelineLayoutData GetPipelineLayoutData(RHIPipelineLayoutHandle handle) const;

        // ========================================================================
        // Render Pass Management (Metal uses MTLRenderPassDescriptor per-draw,
        // but we cache the configuration)
        // ========================================================================
        struct RenderPassData
        {
            TextureFormat colorFormat = TextureFormat::B8G8R8A8_UNORM;
            TextureFormat depthFormat = TextureFormat::Undefined;
            LoadOp colorLoadOp = LoadOp::Clear;
            StoreOp colorStoreOp = StoreOp::Store;
            LoadOp depthLoadOp = LoadOp::Clear;
            StoreOp depthStoreOp = StoreOp::DontCare;
            uint32_t width = 0;
            uint32_t height = 0;
        };

        RHIRenderPassHandle RegisterRenderPass(const RenderPassData& data);
        void UnregisterRenderPass(RHIRenderPassHandle handle);
        RenderPassData GetRenderPassData(RHIRenderPassHandle handle) const;

        // ========================================================================
        // Buffer Management
        // ========================================================================
#ifdef __OBJC__
        struct BufferData
        {
            id<MTLBuffer> buffer = nil;
            uint64_t size = 0;
            bool cpuVisible = false;
        };
#else
        struct BufferData
        {
            void* buffer = nullptr;
            uint64_t size = 0;
            bool cpuVisible = false;
        };
#endif

        RHIBufferHandle RegisterBuffer(const BufferData& data);
        void UnregisterBuffer(RHIBufferHandle handle);
        BufferData GetBufferData(RHIBufferHandle handle) const;

        // ========================================================================
        // Texture Management
        // ========================================================================
#ifdef __OBJC__
        struct TextureData
        {
            id<MTLTexture> texture = nil;
            uint32_t width = 0;
            uint32_t height = 0;
            TextureFormat format = TextureFormat::Undefined;
        };
#else
        struct TextureData
        {
            void* texture = nullptr;
            uint32_t width = 0;
            uint32_t height = 0;
            TextureFormat format = TextureFormat::Undefined;
        };
#endif

        RHITextureHandle RegisterTexture(const TextureData& data);
        void UnregisterTexture(RHITextureHandle handle);
        TextureData GetTextureData(RHITextureHandle handle) const;

        // ========================================================================
        // Sampler Management
        // ========================================================================
#ifdef __OBJC__
        RHISamplerHandle RegisterSampler(id<MTLSamplerState> sampler);
        id<MTLSamplerState> GetSampler(RHISamplerHandle handle) const;
#else
        RHISamplerHandle RegisterSampler(void* sampler);
        void* GetSampler(RHISamplerHandle handle) const;
#endif
        void UnregisterSampler(RHISamplerHandle handle);

        // ========================================================================
        // Shader Module Management
        // ========================================================================
#ifdef __OBJC__
        struct ShaderModuleData
        {
            id<MTLFunction> function = nil;
            ShaderStage stage = ShaderStage::None;
            std::string entryPoint = "main0";
        };
#else
        struct ShaderModuleData
        {
            void* function = nullptr;
            ShaderStage stage = ShaderStage::None;
            std::string entryPoint = "main0";
        };
#endif

        RHIShaderModuleHandle RegisterShaderModule(const ShaderModuleData& data);
        void UnregisterShaderModule(RHIShaderModuleHandle handle);
        ShaderModuleData GetShaderModuleData(RHIShaderModuleHandle handle) const;

        // ========================================================================
        // Descriptor Set Layout Management (Argument Buffer Encoders in Metal)
        // ========================================================================
#ifdef __OBJC__
        struct DescriptorSetLayoutData
        {
            id<MTLArgumentEncoder> encoder = nil;
            NSUInteger encodedLength = 0;
            uint32_t maxTextures = 0;  // For bindless texture arrays
        };
#else
        struct DescriptorSetLayoutData
        {
            void* encoder = nullptr;
            uint64_t encodedLength = 0;
            uint32_t maxTextures = 0;
        };
#endif

        RHIDescriptorSetLayoutHandle RegisterDescriptorSetLayout(const DescriptorSetLayoutData& data);
        void UnregisterDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle);
        DescriptorSetLayoutData GetDescriptorSetLayoutData(RHIDescriptorSetLayoutHandle handle) const;

        // ========================================================================
        // Descriptor Set Management (Argument Buffers in Metal)
        // ========================================================================
#ifdef __OBJC__
        struct DescriptorSetData
        {
            id<MTLBuffer> argumentBuffer = nil;
            id<MTLArgumentEncoder> encoder = nil;
            RHIDescriptorSetLayoutHandle layoutHandle;
        };
#else
        struct DescriptorSetData
        {
            void* argumentBuffer = nullptr;
            void* encoder = nullptr;
            RHIDescriptorSetLayoutHandle layoutHandle;
        };
#endif

        RHIDescriptorSetHandle RegisterDescriptorSet(const DescriptorSetData& data);
        void UnregisterDescriptorSet(RHIDescriptorSetHandle handle);
        DescriptorSetData GetDescriptorSetData(RHIDescriptorSetHandle handle) const;

        // ========================================================================
        // Command Buffer Management
        // ========================================================================
#ifdef __OBJC__
        void SetCurrentCommandBuffer(uint32_t frameIndex, id<MTLCommandBuffer> cmd);
        id<MTLCommandBuffer> GetCommandBuffer(RHICommandBufferHandle handle) const;
#else
        void SetCurrentCommandBuffer(uint32_t frameIndex, void* cmd);
        void* GetCommandBuffer(RHICommandBufferHandle handle) const;
#endif
        RHICommandBufferHandle GetCurrentCommandBufferHandle(uint32_t frameIndex) const;

        // Immediate command buffer (for one-shot operations)
#ifdef __OBJC__
        void SetImmediateCommandBuffer(id<MTLCommandBuffer> cmd);
#else
        void SetImmediateCommandBuffer(void* cmd);
#endif
        RHICommandBufferHandle GetImmediateCommandBufferHandle() const;

        // ========================================================================
        // Cleanup
        // ========================================================================
        void Clear();

    private:
        MetalResourceRegistry() = default;
        ~MetalResourceRegistry() = default;

        uint64_t GenerateId();

        mutable std::mutex m_Mutex;
        uint64_t m_NextId = 1;

        // Resource storage
        std::unordered_map<uint64_t, PipelineData> m_Pipelines;
        std::unordered_map<uint64_t, PipelineLayoutData> m_PipelineLayouts;
        std::unordered_map<uint64_t, RenderPassData> m_RenderPasses;
        std::unordered_map<uint64_t, BufferData> m_Buffers;
        std::unordered_map<uint64_t, TextureData> m_Textures;
        std::unordered_map<uint64_t, ShaderModuleData> m_ShaderModules;
        std::unordered_map<uint64_t, DescriptorSetLayoutData> m_DescriptorSetLayouts;
        std::unordered_map<uint64_t, DescriptorSetData> m_DescriptorSets;

#ifdef __OBJC__
        std::unordered_map<uint64_t, id<MTLSamplerState>> m_Samplers;
#else
        std::unordered_map<uint64_t, void*> m_Samplers;
#endif

        // Command buffer tracking (per frame)
        static constexpr uint32_t MaxFramesInFlight = 2;
#ifdef __OBJC__
        id<MTLCommandBuffer> m_CommandBuffers[MaxFramesInFlight] = {};
        id<MTLCommandBuffer> m_ImmediateCommandBuffer = nil;
#else
        void* m_CommandBuffers[MaxFramesInFlight] = {};
        void* m_ImmediateCommandBuffer = nullptr;
#endif
        uint64_t m_CommandBufferHandleIds[MaxFramesInFlight] = {};
        static constexpr uint64_t ImmediateCommandBufferHandleId = 0xFFFE0000;
    };

}
