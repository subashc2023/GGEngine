#include "ggpch.h"
#include "VertexLayout.h"

namespace GGEngine {

    uint32_t GetVertexAttributeSize(VertexAttributeType type)
    {
        switch (type)
        {
            case VertexAttributeType::Float:     return 4;
            case VertexAttributeType::Float2:    return 8;
            case VertexAttributeType::Float3:    return 12;
            case VertexAttributeType::Float4:    return 16;
            case VertexAttributeType::Int:       return 4;
            case VertexAttributeType::Int2:      return 8;
            case VertexAttributeType::Int3:      return 12;
            case VertexAttributeType::Int4:      return 16;
            case VertexAttributeType::UByte4Norm: return 4;
            case VertexAttributeType::UInt:      return 4;
        }
        return 0;
    }

    TextureFormat GetVertexAttributeFormat(VertexAttributeType type)
    {
        switch (type)
        {
            case VertexAttributeType::Float:     return TextureFormat::R32_SFLOAT;
            case VertexAttributeType::Float2:    return TextureFormat::R32G32_SFLOAT;
            case VertexAttributeType::Float3:    return TextureFormat::R32G32B32_SFLOAT;
            case VertexAttributeType::Float4:    return TextureFormat::R32G32B32A32_SFLOAT;
            case VertexAttributeType::Int:       return TextureFormat::R32_SINT;
            case VertexAttributeType::Int2:      return TextureFormat::R32G32_SINT;
            case VertexAttributeType::Int3:      return TextureFormat::R32G32B32_SINT;
            case VertexAttributeType::Int4:      return TextureFormat::R32G32B32A32_SINT;
            case VertexAttributeType::UByte4Norm: return TextureFormat::R8G8B8A8_UNORM;
            case VertexAttributeType::UInt:      return TextureFormat::R32_UINT;
        }
        return TextureFormat::Undefined;
    }

    VertexAttribute::VertexAttribute(const std::string& name, VertexAttributeType type)
        : name(name), type(type), size(GetVertexAttributeSize(type)), offset(0)
    {
    }

    VertexLayout& VertexLayout::Push(const std::string& name, VertexAttributeType type)
    {
        VertexAttribute attr(name, type);
        attr.offset = m_Stride;
        m_Stride += attr.size;
        m_Attributes.push_back(attr);
        return *this;
    }

    RHIVertexBindingDescription VertexLayout::GetBindingDescription(uint32_t binding) const
    {
        RHIVertexBindingDescription desc{};
        desc.binding = binding;
        desc.stride = m_Stride;
        desc.inputRate = VertexInputRate::Vertex;
        return desc;
    }

    std::vector<RHIVertexAttributeDescription> VertexLayout::GetAttributeDescriptions(uint32_t binding) const
    {
        std::vector<RHIVertexAttributeDescription> descriptions;
        descriptions.reserve(m_Attributes.size());

        for (uint32_t i = 0; i < m_Attributes.size(); ++i)
        {
            RHIVertexAttributeDescription desc{};
            desc.binding = binding;
            desc.location = i;
            desc.format = GetVertexAttributeFormat(m_Attributes[i].type);
            desc.offset = m_Attributes[i].offset;
            descriptions.push_back(desc);
        }

        return descriptions;
    }

}
