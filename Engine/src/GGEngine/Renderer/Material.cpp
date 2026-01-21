#include "ggpch.h"
#include "Material.h"
#include "GGEngine/Asset/Shader.h"
#include "RenderCommand.h"

namespace GGEngine {

    Material::~Material()
    {
        m_Pipeline.reset();
    }

    void Material::RegisterProperty(const std::string& name, PropertyType type,
                                    VkShaderStageFlags stage, uint32_t offset)
    {
        PropertyMetadata metadata;
        metadata.type = type;
        metadata.offset = offset;
        metadata.size = GetPropertyTypeSize(type);
        metadata.stage = stage;

        m_Properties[name] = metadata;

        GG_CORE_TRACE("Material property registered: '{}' (offset: {}, size: {}, stage: {})",
                      name, offset, metadata.size, stage);
    }

    bool Material::Create(const MaterialSpecification& spec)
    {
        if (!spec.shader)
        {
            GG_CORE_ERROR("Material::Create failed: shader is null");
            return false;
        }

        if (spec.renderPass == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("Material::Create failed: renderPass is null");
            return false;
        }

        m_Name = spec.name;
        m_Shader = spec.shader;

        // Build pipeline specification
        PipelineSpecification pipelineSpec;
        pipelineSpec.shader = spec.shader;
        pipelineSpec.renderPass = spec.renderPass;
        pipelineSpec.vertexLayout = spec.vertexLayout;
        pipelineSpec.topology = spec.topology;
        pipelineSpec.cullMode = spec.cullMode;
        pipelineSpec.frontFace = spec.frontFace;
        pipelineSpec.blendMode = spec.blendMode;
        pipelineSpec.depthTestEnable = spec.depthTestEnable;
        pipelineSpec.depthWriteEnable = spec.depthWriteEnable;
        pipelineSpec.descriptorSetLayouts = spec.descriptorSetLayouts;
        pipelineSpec.debugName = spec.name;

        // Build push constant ranges from registered properties
        pipelineSpec.pushConstantRanges = BuildPushConstantRanges();

        // Create the pipeline
        m_Pipeline = CreateScope<Pipeline>(pipelineSpec);

        GG_CORE_INFO("Material '{}' created successfully", m_Name);
        return true;
    }

    std::vector<PushConstantRange> Material::BuildPushConstantRanges() const
    {
        // Group properties by shader stage and find contiguous ranges
        // For simplicity, we create one range per unique stage combination
        // covering from min offset to max offset+size for that stage

        std::unordered_map<VkShaderStageFlags, std::pair<uint32_t, uint32_t>> stageRanges;

        for (const auto& [name, metadata] : m_Properties)
        {
            auto it = stageRanges.find(metadata.stage);
            if (it == stageRanges.end())
            {
                // New stage - initialize with this property's range
                stageRanges[metadata.stage] = { metadata.offset, metadata.offset + metadata.size };
            }
            else
            {
                // Expand range to include this property
                it->second.first = std::min(it->second.first, metadata.offset);
                it->second.second = std::max(it->second.second, metadata.offset + metadata.size);
            }
        }

        std::vector<PushConstantRange> ranges;
        for (const auto& [stage, range] : stageRanges)
        {
            PushConstantRange pcRange;
            pcRange.stageFlags = stage;
            pcRange.offset = range.first;
            pcRange.size = range.second - range.first;
            ranges.push_back(pcRange);
        }

        return ranges;
    }

    bool Material::ValidateProperty(const std::string& name, PropertyType expectedType) const
    {
        auto it = m_Properties.find(name);
        if (it == m_Properties.end())
        {
            GG_CORE_WARN("Material '{}': property '{}' not found", m_Name, name);
            return false;
        }

        if (it->second.type != expectedType)
        {
            GG_CORE_WARN("Material '{}': property '{}' type mismatch (expected {}, got {})",
                         m_Name, name, static_cast<int>(expectedType), static_cast<int>(it->second.type));
            return false;
        }

        return true;
    }

    void Material::WriteProperty(uint32_t offset, const void* data, uint32_t size)
    {
        if (offset + size > m_PushConstantBuffer.size())
        {
            GG_CORE_ERROR("Material '{}': property write out of bounds (offset: {}, size: {})",
                          m_Name, offset, size);
            return;
        }

        std::memcpy(m_PushConstantBuffer.data() + offset, data, size);
    }

    void Material::SetFloat(const std::string& name, float value)
    {
        if (!ValidateProperty(name, PropertyType::Float))
            return;

        const auto& metadata = m_Properties.at(name);
        WriteProperty(metadata.offset, &value, sizeof(float));
    }

    void Material::SetVec2(const std::string& name, float x, float y)
    {
        float values[2] = { x, y };
        SetVec2(name, values);
    }

    void Material::SetVec2(const std::string& name, const float* values)
    {
        if (!ValidateProperty(name, PropertyType::Vec2))
            return;

        const auto& metadata = m_Properties.at(name);
        WriteProperty(metadata.offset, values, sizeof(float) * 2);
    }

    void Material::SetVec3(const std::string& name, float x, float y, float z)
    {
        float values[3] = { x, y, z };
        SetVec3(name, values);
    }

    void Material::SetVec3(const std::string& name, const float* values)
    {
        if (!ValidateProperty(name, PropertyType::Vec3))
            return;

        const auto& metadata = m_Properties.at(name);
        WriteProperty(metadata.offset, values, sizeof(float) * 3);
    }

    void Material::SetVec4(const std::string& name, float x, float y, float z, float w)
    {
        float values[4] = { x, y, z, w };
        SetVec4(name, values);
    }

    void Material::SetVec4(const std::string& name, const float* values)
    {
        if (!ValidateProperty(name, PropertyType::Vec4))
            return;

        const auto& metadata = m_Properties.at(name);
        WriteProperty(metadata.offset, values, sizeof(float) * 4);
    }

    void Material::SetMat4(const std::string& name, const Mat4& matrix)
    {
        if (!ValidateProperty(name, PropertyType::Mat4))
            return;

        const auto& metadata = m_Properties.at(name);
        WriteProperty(metadata.offset, matrix.data, sizeof(float) * 16);
    }

    bool Material::HasProperty(const std::string& name) const
    {
        return m_Properties.find(name) != m_Properties.end();
    }

    const PropertyMetadata* Material::GetPropertyMetadata(const std::string& name) const
    {
        auto it = m_Properties.find(name);
        if (it == m_Properties.end())
            return nullptr;
        return &it->second;
    }

    void Material::Bind(VkCommandBuffer cmd) const
    {
        if (!m_Pipeline)
        {
            GG_CORE_ERROR("Material '{}': cannot bind - pipeline not created", m_Name);
            return;
        }

        // Bind the pipeline
        m_Pipeline->Bind(cmd);

        // Push constants for each stage
        // Group properties by stage and push the buffer region for each
        VkPipelineLayout layout = m_Pipeline->GetLayout();

        // Get the push constant ranges we built during Create()
        const auto& spec = m_Pipeline->GetSpecification();
        for (const auto& range : spec.pushConstantRanges)
        {
            vkCmdPushConstants(
                cmd,
                layout,
                range.stageFlags,
                range.offset,
                range.size,
                m_PushConstantBuffer.data() + range.offset
            );
        }
    }

    VkPipelineLayout Material::GetPipelineLayout() const
    {
        if (!m_Pipeline)
            return VK_NULL_HANDLE;
        return m_Pipeline->GetLayout();
    }

}
