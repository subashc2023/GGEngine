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
        GG_CORE_TRACE("ShaderLibrary initializing...");

        // Load built-in shaders that ship with the engine
        Load("basic", "assets/shaders/compiled/basic");
        Load("texture", "assets/shaders/compiled/texture");
        Load("quad2d", "assets/shaders/compiled/quad2d");

        GG_CORE_INFO("ShaderLibrary initialized with {} built-in shaders", m_Shaders.size());
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

    void ShaderLibrary::Add(const std::string& name, AssetHandle<Shader> shader)
    {
        if (!shader.IsValid())
        {
            GG_CORE_ERROR("Cannot add invalid shader with name '{}'", name);
            return;
        }

        auto it = m_Shaders.find(name);
        if (it != m_Shaders.end())
        {
            GG_CORE_WARN("Overwriting existing shader '{}'", name);
        }

        m_Shaders[name] = shader;
        shader.Get()->SetName(name);
        GG_CORE_TRACE("Added shader '{}' to library", name);
    }

    void ShaderLibrary::Add(AssetHandle<Shader> shader)
    {
        if (!shader.IsValid())
        {
            GG_CORE_ERROR("Cannot add invalid shader");
            return;
        }

        const std::string& name = shader.Get()->GetName();
        if (name.empty())
        {
            GG_CORE_ERROR("Cannot add shader without a name - use Add(name, shader) instead");
            return;
        }

        Add(name, shader);
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
