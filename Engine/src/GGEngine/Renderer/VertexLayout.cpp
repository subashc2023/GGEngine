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
        }
        return 0;
    }

    VkFormat GetVertexAttributeFormat(VertexAttributeType type)
    {
        switch (type)
        {
            case VertexAttributeType::Float:     return VK_FORMAT_R32_SFLOAT;
            case VertexAttributeType::Float2:    return VK_FORMAT_R32G32_SFLOAT;
            case VertexAttributeType::Float3:    return VK_FORMAT_R32G32B32_SFLOAT;
            case VertexAttributeType::Float4:    return VK_FORMAT_R32G32B32A32_SFLOAT;
            case VertexAttributeType::Int:       return VK_FORMAT_R32_SINT;
            case VertexAttributeType::Int2:      return VK_FORMAT_R32G32_SINT;
            case VertexAttributeType::Int3:      return VK_FORMAT_R32G32B32_SINT;
            case VertexAttributeType::Int4:      return VK_FORMAT_R32G32B32A32_SINT;
            case VertexAttributeType::UByte4Norm: return VK_FORMAT_R8G8B8A8_UNORM;
        }
        return VK_FORMAT_UNDEFINED;
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

    VkVertexInputBindingDescription VertexLayout::GetBindingDescription(uint32_t binding) const
    {
        VkVertexInputBindingDescription desc{};
        desc.binding = binding;
        desc.stride = m_Stride;
        desc.inputRate = VK_VERTEX_INPUT_RATE_VERTEX;
        return desc;
    }

    std::vector<VkVertexInputAttributeDescription> VertexLayout::GetAttributeDescriptions(uint32_t binding) const
    {
        std::vector<VkVertexInputAttributeDescription> descriptions;
        descriptions.reserve(m_Attributes.size());

        for (uint32_t i = 0; i < m_Attributes.size(); ++i)
        {
            VkVertexInputAttributeDescription desc{};
            desc.binding = binding;
            desc.location = i;
            desc.format = GetVertexAttributeFormat(m_Attributes[i].type);
            desc.offset = m_Attributes[i].offset;
            descriptions.push_back(desc);
        }

        return descriptions;
    }

}
