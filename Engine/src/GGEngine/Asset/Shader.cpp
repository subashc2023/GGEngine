#include "ggpch.h"
#include "Shader.h"
#include "GGEngine/Core/Profiler.h"
#include "AssetManager.h"
#include "ShaderLibrary.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "Platform/Vulkan/VulkanRHI.h"

#include <filesystem>

namespace GGEngine {

    AssetHandle<Shader> Shader::Create(const std::string& path)
    {
        return AssetManager::Get().Load<Shader>(path);
    }

    AssetHandle<Shader> Shader::Create(const std::string& name, const std::string& path)
    {
        return ShaderLibrary::Get().Load(name, path);
    }

    Shader::~Shader()
    {
        Unload();
    }

    bool Shader::Load(const std::string& basePath)
    {
        GG_PROFILE_FUNCTION();
        // basePath like "assets/shaders/compiled/triangle" -> loads triangle.vert.spv, triangle.frag.spv
        bool hasVertex = LoadStageFromFile(ShaderStage::Vertex, basePath + ".vert.spv");
        bool hasFragment = LoadStageFromFile(ShaderStage::Fragment, basePath + ".frag.spv");

        // Optional stages - silently skip if not found
        LoadStageFromFile(ShaderStage::Geometry, basePath + ".geom.spv");
        LoadStageFromFile(ShaderStage::Compute, basePath + ".comp.spv");

        bool loadedAny = hasVertex || hasFragment || !m_Stages.empty();
        m_Loaded = loadedAny;
        m_Path = basePath;

        // Extract name from path if not already set
        if (m_Name.empty())
        {
            std::filesystem::path fsPath(basePath);
            m_Name = fsPath.stem().string();
        }

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

    bool Shader::LoadStage(ShaderStage stage, const std::vector<char>& spirvCode)
    {
        GG_PROFILE_FUNCTION();
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
                // Unregister and destroy old module
                auto& registry = VulkanResourceRegistry::Get();
                VkShaderModule oldModule = registry.GetShaderModule(it->handle);
                if (oldModule != VK_NULL_HANDLE)
                {
                    vkDestroyShaderModule(device, oldModule, nullptr);
                }
                registry.UnregisterShaderModule(it->handle);
                m_Stages.erase(it);
                break;
            }
        }

        // Register the new module
        auto& registry = VulkanResourceRegistry::Get();
        RHIShaderModuleHandle handle = registry.RegisterShaderModule(module, stage, "main");

        ShaderStageInfo stageInfo;
        stageInfo.stage = stage;
        stageInfo.handle = handle;
        stageInfo.entryPoint = "main";
        m_Stages.push_back(stageInfo);

        return true;
    }

    bool Shader::LoadStageFromFile(ShaderStage stage, const std::string& path)
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

        // Check if VulkanContext is still valid (may be destroyed during static destruction)
        VkDevice device = VulkanContext::Get().GetDevice();
        if (device == VK_NULL_HANDLE)
        {
            m_Stages.clear();
            m_Loaded = false;
            return;
        }

        auto& registry = VulkanResourceRegistry::Get();
        for (auto& stageInfo : m_Stages)
        {
            VkShaderModule module = registry.GetShaderModule(stageInfo.handle);
            if (module != VK_NULL_HANDLE)
            {
                vkDestroyShaderModule(device, module, nullptr);
            }
            registry.UnregisterShaderModule(stageInfo.handle);
        }
        m_Stages.clear();
        m_Loaded = false;
    }

    bool Shader::HasStage(ShaderStage stage) const
    {
        for (const auto& s : m_Stages)
        {
            if (s.stage == stage)
                return true;
        }
        return false;
    }

    RHIShaderModuleHandle Shader::GetStageHandle(ShaderStage stage) const
    {
        for (const auto& s : m_Stages)
        {
            if (s.stage == stage)
                return s.handle;
        }
        return NullShaderModule;
    }

}
