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

    Result<void> Shader::Load(const std::string& basePath)
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
        m_SourcePath = basePath;  // Store for hot reload

        // Extract name from path if not already set
        if (m_Name.empty())
        {
            std::filesystem::path fsPath(basePath);
            m_Name = fsPath.stem().string();
        }

        if (loadedAny)
        {
            GG_CORE_TRACE("Shader loaded: {} ({} stages)", basePath, m_Stages.size());
            return Result<void>::Ok();
        }

        return Result<void>::Err("Shader failed to load any stages: " + basePath);
    }

    Result<void> Shader::LoadStage(ShaderStage stage, const std::vector<char>& spirvCode)
    {
        GG_PROFILE_FUNCTION();
        if (spirvCode.empty())
            return Result<void>::Err("Empty SPIR-V code");

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
        auto result = device.TryCreateShaderModule(stage, spirvCode);
        if (result.IsErr())
        {
            return Result<void>::Err("Failed to create shader module: " + result.Error());
        }

        ShaderStageInfo stageInfo;
        stageInfo.stage = stage;
        stageInfo.handle = result.Value();
        stageInfo.entryPoint = "main";
        m_Stages.push_back(stageInfo);

        return Result<void>::Ok();
    }

    bool Shader::LoadStageFromFile(ShaderStage stage, const std::string& path)
    {
        auto code = AssetManager::Get().ReadFileRaw(path);
        if (code.empty())
        {
            // Not an error - stage might not exist (e.g., no geometry shader)
            return false;
        }
        auto result = LoadStage(stage, code);
        if (result.IsErr())
        {
            GG_CORE_ERROR("Failed to load shader stage from {}: {}", path, result.Error());
            return false;
        }
        return true;
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

#ifndef GG_DIST
    Result<void> Shader::Reload()
    {
        GG_PROFILE_SCOPE("Shader::Reload");

        if (m_SourcePath.empty())
        {
            return Result<void>::Err("Cannot reload shader without source path");
        }

        GG_CORE_INFO("Hot reloading shader: {}", m_SourcePath);

        SetState(AssetState::Reloading);

        // Destroy all existing shader modules
        auto& device = RHIDevice::Get();
        for (auto& stageInfo : m_Stages)
        {
            device.DestroyShaderModule(stageInfo.handle);
        }
        m_Stages.clear();

        // Reload all stages from disk
        bool hasVertex = LoadStageFromFile(ShaderStage::Vertex, m_SourcePath + ".vert.spv");
        bool hasFragment = LoadStageFromFile(ShaderStage::Fragment, m_SourcePath + ".frag.spv");

        // Optional stages
        LoadStageFromFile(ShaderStage::Geometry, m_SourcePath + ".geom.spv");
        LoadStageFromFile(ShaderStage::Compute, m_SourcePath + ".comp.spv");

        bool loadedAny = hasVertex || hasFragment || !m_Stages.empty();

        if (loadedAny)
        {
            SetState(AssetState::Ready);
            GG_CORE_INFO("Hot reload complete: {} ({} stages)", m_SourcePath, m_Stages.size());
            return Result<void>::Ok();
        }
        else
        {
            SetState(AssetState::Failed);
            return Result<void>::Err("Hot reload failed: no stages loaded for " + m_SourcePath);
        }
    }
#endif

}
