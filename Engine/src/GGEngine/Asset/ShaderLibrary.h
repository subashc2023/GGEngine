#pragma once

#include "GGEngine/Core/Core.h"
#include "AssetManager.h"
#include "Shader.h"

#include <string>
#include <unordered_map>

namespace GGEngine {

    // Convenience class for managing shaders with friendly names
    class GG_API ShaderLibrary
    {
    public:
        static ShaderLibrary& Get();

        void Init();
        void Shutdown();

        // Load shader and register with a name
        AssetHandle<Shader> Load(const std::string& name, const std::string& path);

        // Load shader using filename as name (extracts last component of path)
        AssetHandle<Shader> Load(const std::string& path);

        // Get shader by name
        AssetHandle<Shader> Get(const std::string& name) const;

        // Check if shader exists by name
        bool Exists(const std::string& name) const;

    private:
        ShaderLibrary() = default;
        ~ShaderLibrary() = default;
        ShaderLibrary(const ShaderLibrary&) = delete;
        ShaderLibrary& operator=(const ShaderLibrary&) = delete;

        std::unordered_map<std::string, AssetHandle<Shader>> m_Shaders;
    };

}
