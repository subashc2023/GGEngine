#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHIEnums.h"
#include <cstdint>

namespace GGEngine {

    // ============================================================================
    // RHI Handle Types
    // ============================================================================
    // Opaque handles that abstract backend-specific resources.
    // The actual GPU resources are stored in a backend-specific registry
    // and looked up by handle ID at runtime.
    //
    // Benefits:
    // - No virtual function overhead
    // - Backend types don't leak into public headers
    // - Handles can be validated (IsValid check)
    // - Easy to serialize/deserialize
    // ============================================================================

    // Command buffer for recording GPU commands
    struct GG_API RHICommandBufferHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHICommandBufferHandle& other) const { return id == other.id; }
        bool operator!=(const RHICommandBufferHandle& other) const { return id != other.id; }
    };

    // Graphics/compute pipeline
    struct GG_API RHIPipelineHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIPipelineHandle& other) const { return id == other.id; }
        bool operator!=(const RHIPipelineHandle& other) const { return id != other.id; }
    };

    // Pipeline layout (describes push constants and descriptor set layouts)
    struct GG_API RHIPipelineLayoutHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIPipelineLayoutHandle& other) const { return id == other.id; }
        bool operator!=(const RHIPipelineLayoutHandle& other) const { return id != other.id; }
    };

    // Render pass (defines attachments and subpasses)
    struct GG_API RHIRenderPassHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIRenderPassHandle& other) const { return id == other.id; }
        bool operator!=(const RHIRenderPassHandle& other) const { return id != other.id; }
    };

    // Framebuffer (render target with attachments)
    struct GG_API RHIFramebufferHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIFramebufferHandle& other) const { return id == other.id; }
        bool operator!=(const RHIFramebufferHandle& other) const { return id != other.id; }
    };

    // GPU buffer (vertex, index, uniform, storage)
    struct GG_API RHIBufferHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIBufferHandle& other) const { return id == other.id; }
        bool operator!=(const RHIBufferHandle& other) const { return id != other.id; }
    };

    // Texture/image resource
    struct GG_API RHITextureHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHITextureHandle& other) const { return id == other.id; }
        bool operator!=(const RHITextureHandle& other) const { return id != other.id; }
    };

    // Sampler state
    struct GG_API RHISamplerHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHISamplerHandle& other) const { return id == other.id; }
        bool operator!=(const RHISamplerHandle& other) const { return id != other.id; }
    };

    // Shader program (collection of shader modules)
    struct GG_API RHIShaderHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIShaderHandle& other) const { return id == other.id; }
        bool operator!=(const RHIShaderHandle& other) const { return id != other.id; }
    };

    // Individual shader module (one stage: vertex, fragment, etc.)
    struct GG_API RHIShaderModuleHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIShaderModuleHandle& other) const { return id == other.id; }
        bool operator!=(const RHIShaderModuleHandle& other) const { return id != other.id; }
    };

    // Descriptor set layout (template for resource bindings)
    struct GG_API RHIDescriptorSetLayoutHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIDescriptorSetLayoutHandle& other) const { return id == other.id; }
        bool operator!=(const RHIDescriptorSetLayoutHandle& other) const { return id != other.id; }
    };

    // Descriptor set (actual resource bindings)
    struct GG_API RHIDescriptorSetHandle
    {
        uint64_t id = 0;
        bool IsValid() const { return id != 0; }
        bool operator==(const RHIDescriptorSetHandle& other) const { return id == other.id; }
        bool operator!=(const RHIDescriptorSetHandle& other) const { return id != other.id; }
    };

    // ============================================================================
    // Null Handle Constants
    // ============================================================================

    constexpr RHICommandBufferHandle NullCommandBuffer{0};
    constexpr RHIPipelineHandle NullPipeline{0};
    constexpr RHIPipelineLayoutHandle NullPipelineLayout{0};
    constexpr RHIRenderPassHandle NullRenderPass{0};
    constexpr RHIFramebufferHandle NullFramebuffer{0};
    constexpr RHIBufferHandle NullBuffer{0};
    constexpr RHITextureHandle NullTexture{0};
    constexpr RHISamplerHandle NullSampler{0};
    constexpr RHIShaderHandle NullShader{0};
    constexpr RHIShaderModuleHandle NullShaderModule{0};
    constexpr RHIDescriptorSetLayoutHandle NullDescriptorSetLayout{0};
    constexpr RHIDescriptorSetHandle NullDescriptorSet{0};

    // ============================================================================
    // Vertex Input Structures
    // ============================================================================

    // Vertex input rate (per-vertex or per-instance)
    enum class VertexInputRate : uint8_t
    {
        Vertex,
        Instance
    };

    // Vertex binding description (describes how to interpret a vertex buffer)
    struct GG_API RHIVertexBindingDescription
    {
        uint32_t binding = 0;
        uint32_t stride = 0;
        VertexInputRate inputRate = VertexInputRate::Vertex;
    };

    // Vertex attribute description (describes a single vertex attribute)
    struct GG_API RHIVertexAttributeDescription
    {
        uint32_t location = 0;
        uint32_t binding = 0;
        TextureFormat format = TextureFormat::R32G32B32A32_SFLOAT;
        uint32_t offset = 0;
    };

}

// Hash support for using handles as map keys
namespace std {
    template<> struct hash<GGEngine::RHICommandBufferHandle> {
        size_t operator()(const GGEngine::RHICommandBufferHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIPipelineHandle> {
        size_t operator()(const GGEngine::RHIPipelineHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIPipelineLayoutHandle> {
        size_t operator()(const GGEngine::RHIPipelineLayoutHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIRenderPassHandle> {
        size_t operator()(const GGEngine::RHIRenderPassHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIFramebufferHandle> {
        size_t operator()(const GGEngine::RHIFramebufferHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIBufferHandle> {
        size_t operator()(const GGEngine::RHIBufferHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHITextureHandle> {
        size_t operator()(const GGEngine::RHITextureHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHISamplerHandle> {
        size_t operator()(const GGEngine::RHISamplerHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIShaderHandle> {
        size_t operator()(const GGEngine::RHIShaderHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIShaderModuleHandle> {
        size_t operator()(const GGEngine::RHIShaderModuleHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIDescriptorSetLayoutHandle> {
        size_t operator()(const GGEngine::RHIDescriptorSetLayoutHandle& h) const { return hash<uint64_t>()(h.id); }
    };
    template<> struct hash<GGEngine::RHIDescriptorSetHandle> {
        size_t operator()(const GGEngine::RHIDescriptorSetHandle& h) const { return hash<uint64_t>()(h.id); }
    };
}
