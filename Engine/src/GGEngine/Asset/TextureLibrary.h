#pragma once

#include "GGEngine/Core/Core.h"
#include "AssetManager.h"
#include "Texture.h"

#include <string>
#include <vector>
#include <unordered_map>

namespace GGEngine {

    // Convenience class for managing textures with friendly names
    class GG_API TextureLibrary
    {
    public:
        static TextureLibrary& Get();

        void Init();
        void Shutdown();

        // Load texture and register with a name
        AssetHandle<Texture> Load(const std::string& name, const std::string& path);

        // Load texture using filename as name
        AssetHandle<Texture> Load(const std::string& path);

        // Scan a directory for image files and load them
        void ScanDirectory(const std::string& directory);

        // Add a pre-loaded texture with a specific name
        void Add(const std::string& name, AssetHandle<Texture> texture);

        // Get texture by name (returns invalid handle for built-ins)
        AssetHandle<Texture> Get(const std::string& name) const;

        // Get texture pointer by name (works for both loaded and built-in)
        Texture* GetTexturePtr(const std::string& name) const;

        // Check if texture exists by name
        bool Exists(const std::string& name) const;

        // Check if name is a built-in texture
        bool IsBuiltIn(const std::string& name) const;

        // Get all texture names (for UI dropdown) - includes built-ins
        std::vector<std::string> GetAllNames() const;

        // Get all loaded textures (not including built-ins)
        const std::unordered_map<std::string, AssetHandle<Texture>>& GetAll() const { return m_Textures; }

    private:
        TextureLibrary() = default;
        ~TextureLibrary() = default;
        TextureLibrary(const TextureLibrary&) = delete;
        TextureLibrary& operator=(const TextureLibrary&) = delete;

        void CreateBuiltInTextures();

        std::unordered_map<std::string, AssetHandle<Texture>> m_Textures;
        std::unordered_map<std::string, Scope<Texture>> m_BuiltInTextures;
    };

}
