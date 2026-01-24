#include "ggpch.h"
#include "Shader.h"
#include "GGEngine/Core/Profiler.h"
#include "AssetManager.h"
#include "ShaderLibrary.h"
#include "GGEngine/RHI/RHIDevice.h"

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
        SetState(loadedAny ? AssetState::Ready : AssetState::Failed);
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

        auto& device = RHIDevice::Get();

        // Remove existing stage if present (for hot-reloading support)
        for (auto it = m_Stages.begin(); it != m_Stages.end(); ++it)
        {
            if (it->stage == stage)
            {
                // Destroy old module
                device.DestroyShaderModule(it->handle);
                m_Stages.erase(it);
                break;
            }
        }

        // Create new shader module through RHI device
        RHIShaderModuleHandle handle = device.CreateShaderModule(stage, spirvCode);
        if (!handle.IsValid())
        {
            GG_CORE_ERROR("Failed to create shader module");
            return false;
        }

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

        auto& device = RHIDevice::Get();
        for (auto& stageInfo : m_Stages)
        {
            device.DestroyShaderModule(stageInfo.handle);
        }
        m_Stages.clear();
        SetState(AssetState::Unloaded);
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
