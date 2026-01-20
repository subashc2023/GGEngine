#include "ggpch.h"
#include "ShaderLibrary.h"
#include "AssetManager.h"

#include <filesystem>

namespace GGEngine {

    ShaderLibrary& ShaderLibrary::Get()
    {
        static ShaderLibrary instance;
        return instance;
    }

    void ShaderLibrary::Init()
    {
        GG_CORE_TRACE("ShaderLibrary initialized");
    }

    void ShaderLibrary::Shutdown()
    {
        m_Shaders.clear();
        GG_CORE_TRACE("ShaderLibrary shutdown");
    }

    AssetHandle<Shader> ShaderLibrary::Load(const std::string& name, const std::string& path)
    {
        // Check if already loaded by this name
        auto it = m_Shaders.find(name);
        if (it != m_Shaders.end() && it->second.IsValid())
        {
            GG_CORE_TRACE("Shader '{}' already loaded", name);
            return it->second;
        }

        // Load via AssetManager
        auto handle = AssetManager::Get().Load<Shader>(path);
        if (handle.IsValid())
        {
            m_Shaders[name] = handle;
            GG_CORE_INFO("Shader '{}' loaded from '{}'", name, path);
        }
        else
        {
            GG_CORE_ERROR("Failed to load shader '{}' from '{}'", name, path);
        }

        return handle;
    }

    AssetHandle<Shader> ShaderLibrary::Load(const std::string& path)
    {
        // Extract name from path (last component without extension)
        std::filesystem::path fsPath(path);
        std::string name = fsPath.stem().string();
        return Load(name, path);
    }

    AssetHandle<Shader> ShaderLibrary::Get(const std::string& name) const
    {
        auto it = m_Shaders.find(name);
        if (it != m_Shaders.end())
        {
            return it->second;
        }
        return AssetHandle<Shader>();
    }

    bool ShaderLibrary::Exists(const std::string& name) const
    {
        auto it = m_Shaders.find(name);
        return it != m_Shaders.end() && it->second.IsValid();
    }

}
