#pragma once

#include "Asset.h"
#include "GGEngine/Core.h"

#include <vulkan/vulkan.h>
#include <string>
#include <vector>

namespace GGEngine {

    // Template specialization for Shader type
    class Shader;
    template<>
    constexpr AssetType GetAssetType<Shader>() { return AssetType::Shader; }

    // Represents a compiled SPIR-V shader stage
    struct GG_API ShaderStage
    {
        VkShaderStageFlagBits stage = VK_SHADER_STAGE_FLAG_BITS_MAX_ENUM;
        VkShaderModule module = VK_NULL_HANDLE;
        std::string entryPoint = "main";
    };

    // A shader asset that can contain multiple stages (vert, frag, etc.)
    class GG_API Shader : public Asset
    {
    public:
        Shader() = default;
        ~Shader();

        AssetType GetType() const override { return AssetType::Shader; }

        // Load from SPIR-V files (e.g., "assets/shaders/compiled/triangle" loads triangle.vert.spv and triangle.frag.spv)
        bool Load(const std::string& basePath);

        // Load individual stages
        bool LoadStage(VkShaderStageFlagBits stage, const std::vector<char>& spirvCode);
        bool LoadStageFromFile(VkShaderStageFlagBits stage, const std::string& path);

        void Unload() override;

        const std::vector<ShaderStage>& GetStages() const { return m_Stages; }
        bool HasStage(VkShaderStageFlagBits stage) const;
        VkShaderModule GetStageModule(VkShaderStageFlagBits stage) const;

        // Create pipeline shader stage create infos - ready for VkGraphicsPipelineCreateInfo
        std::vector<VkPipelineShaderStageCreateInfo> GetPipelineStageCreateInfos() const;

    private:
        std::vector<ShaderStage> m_Stages;
    };

}
