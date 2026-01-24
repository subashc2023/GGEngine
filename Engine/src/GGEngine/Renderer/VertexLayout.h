#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include <string>
#include <vector>
#include <cstdint>

namespace GGEngine {

    // Supported vertex attribute types
    enum class VertexAttributeType
    {
        Float,      // R32_SFLOAT
        Float2,     // R32G32_SFLOAT
        Float3,     // R32G32B32_SFLOAT
        Float4,     // R32G32B32A32_SFLOAT
        Int,        // R32_SINT
        Int2,       // R32G32_SINT
        Int3,       // R32G32B32_SINT
        Int4,       // R32G32B32A32_SINT
        UByte4Norm, // R8G8B8A8_UNORM (for colors)
        UInt        // R32_UINT (for bindless texture indices)
    };

    // Returns size in bytes for each attribute type
    GG_API uint32_t GetVertexAttributeSize(VertexAttributeType type);

    // Returns TextureFormat for each attribute type
    GG_API TextureFormat GetVertexAttributeFormat(VertexAttributeType type);

    // Single vertex attribute description
    struct GG_API VertexAttribute
    {
        std::string name;
        VertexAttributeType type;
        uint32_t size;
        uint32_t offset;

        VertexAttribute(const std::string& name, VertexAttributeType type);
    };

    // Describes the layout of vertex data
    class GG_API VertexLayout
    {
    public:
        VertexLayout() = default;

        // Builder pattern for adding attributes
        VertexLayout& Push(const std::string& name, VertexAttributeType type);

        // Accessors
        const std::vector<VertexAttribute>& GetAttributes() const { return m_Attributes; }
        uint32_t GetStride() const { return m_Stride; }
        bool IsEmpty() const { return m_Attributes.empty(); }

        // RHI descriptors
        RHIVertexBindingDescription GetBindingDescription(uint32_t binding = 0) const;
        RHIVertexBindingDescription GetBindingDescription(uint32_t binding, VertexInputRate inputRate) const;
        std::vector<RHIVertexAttributeDescription> GetAttributeDescriptions(uint32_t binding = 0) const;
        std::vector<RHIVertexAttributeDescription> GetAttributeDescriptions(uint32_t binding, uint32_t startLocation) const;

    private:
        std::vector<VertexAttribute> m_Attributes;
        uint32_t m_Stride = 0;
    };

}
