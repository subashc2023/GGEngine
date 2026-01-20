#include "ggpch.h"
#include "Shader.h"
#include "AssetManager.h"
#include "Platform/Vulkan/VulkanContext.h"

namespace GGEngine {

    Shader::~Shader()
    {
        Unload();
    }

    bool Shader::Load(const std::string& basePath)
    {
        // basePath like "assets/shaders/compiled/triangle" -> loads triangle.vert.spv, triangle.frag.spv
        bool hasVertex = LoadStageFromFile(VK_SHADER_STAGE_VERTEX_BIT, basePath + ".vert.spv");
        bool hasFragment = LoadStageFromFile(VK_SHADER_STAGE_FRAGMENT_BIT, basePath + ".frag.spv");

        // Optional stages - silently skip if not found
        LoadStageFromFile(VK_SHADER_STAGE_GEOMETRY_BIT, basePath + ".geom.spv");
        LoadStageFromFile(VK_SHADER_STAGE_COMPUTE_BIT, basePath + ".comp.spv");

        bool loadedAny = hasVertex || hasFragment || !m_Stages.empty();
        m_Loaded = loadedAny;
        m_Path = basePath;

        if (loadedAny)
        {
            GG_CORE_TRACE("Shader loaded: {} ({} stages)", basePath, m_Stages.size());
        }
        else
        {
            GG_CORE_ERROR("Shader failed to load any stages: {}", basePath);
        }

        return loadedAny;
    }

    bool Shader::LoadStage(VkShaderStageFlagBits stage, const std::vector<char>& spirvCode)
    {
        if (spirvCode.empty())
            return false;

        VkShaderModuleCreateInfo createInfo{};
        createInfo.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO;
        createInfo.codeSize = spirvCode.size();
        createInfo.pCode = reinterpret_cast<const uint32_t*>(spirvCode.data());

        VkShaderModule module;
        VkDevice device = VulkanContext::Get().GetDevice();

        if (vkCreateShaderModule(device, &createInfo, nullptr, &module) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create shader module");
            return false;
        }

        // Remove existing stage if present (for hot-reloading support)
        for (auto it = m_Stages.begin(); it != m_Stages.end(); ++it)
        {
            if (it->stage == stage)
            {
                vkDestroyShaderModule(device, it->module, nullptr);
                m_Stages.erase(it);
                break;
            }
        }

        m_Stages.push_back({ stage, module, "main" });
        return true;
    }

    bool Shader::LoadStageFromFile(VkShaderStageFlagBits stage, const std::string& path)
    {
        auto code = AssetManager::Get().ReadFileRaw(path);
        if (code.empty())
        {
            // Not an error - stage might not exist (e.g., no geometry shader)
            return false;
        }
        return LoadStage(stage, code);
    }

    void Shader::Unload()
    {
        if (m_Stages.empty())
            return;

        VkDevice device = VulkanContext::Get().GetDevice();

        for (auto& stage : m_Stages)
        {
            if (stage.module != VK_NULL_HANDLE)
            {
                vkDestroyShaderModule(device, stage.module, nullptr);
                stage.module = VK_NULL_HANDLE;
            }
        }
        m_Stages.clear();
        m_Loaded = false;
    }

    bool Shader::HasStage(VkShaderStageFlagBits stage) const
    {
        for (const auto& s : m_Stages)
        {
            if (s.stage == stage)
                return true;
        }
        return false;
    }

    VkShaderModule Shader::GetStageModule(VkShaderStageFlagBits stage) const
    {
        for (const auto& s : m_Stages)
        {
            if (s.stage == stage)
                return s.module;
        }
        return VK_NULL_HANDLE;
    }

    std::vector<VkPipelineShaderStageCreateInfo> Shader::GetPipelineStageCreateInfos() const
    {
        std::vector<VkPipelineShaderStageCreateInfo> infos;
        infos.reserve(m_Stages.size());

        for (const auto& stage : m_Stages)
        {
            VkPipelineShaderStageCreateInfo info{};
            info.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
            info.stage = stage.stage;
            info.module = stage.module;
            info.pName = stage.entryPoint.c_str();
            infos.push_back(info);
        }

        return infos;
    }

}
