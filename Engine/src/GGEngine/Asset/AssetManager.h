#pragma once

#include "GGEngine/Core.h"
#include "Asset.h"
#include "AssetHandle.h"

#include <filesystem>
#include <unordered_map>
#include <memory>
#include <string>
#include <vector>

namespace GGEngine {

    class GG_API AssetManager
    {
    public:
        static AssetManager& Get();

        void Init();
        void Shutdown();

        // Set custom asset root (optional - auto-detected if not set)
        void SetAssetRoot(const std::filesystem::path& root);
        const std::filesystem::path& GetAssetRoot() const { return m_AssetRoot; }

        // Resolve a relative asset path to absolute path
        std::filesystem::path ResolvePath(const std::string& relativePath) const;

        // Load an asset and return a handle
        template<typename T>
        AssetHandle<T> Load(const std::string& path);

        // Get an already-loaded asset (returns invalid handle if not loaded)
        template<typename T>
        AssetHandle<T> GetHandle(const std::string& path) const;

        // Get asset by ID (used by AssetHandle)
        template<typename T>
        T* GetAssetByID(AssetID id) const;

        // Check generation for handle validity
        uint32_t GetGeneration(AssetID id) const;

        // Check if asset is loaded
        bool IsLoaded(const std::string& path) const;

        // Unload a specific asset
        void Unload(const std::string& path);

        // Unload all assets
        void UnloadAll();

        // Read raw file data (utility for loaders)
        std::vector<char> ReadFileRaw(const std::string& relativePath) const;
        std::vector<char> ReadFileRawAbsolute(const std::filesystem::path& absolutePath) const;

    private:
        AssetManager() = default;
        ~AssetManager() = default;
        AssetManager(const AssetManager&) = delete;
        AssetManager& operator=(const AssetManager&) = delete;

        void DetectAssetRoot();

        std::filesystem::path m_AssetRoot;
        std::unordered_map<std::string, std::shared_ptr<Asset>> m_Assets;
        std::unordered_map<AssetID, std::shared_ptr<Asset>> m_AssetsByID;
        std::unordered_map<AssetID, uint32_t> m_Generations;
        AssetID m_NextID = 1;
    };

    // Template implementations

    template<typename T>
    AssetHandle<T> AssetManager::Load(const std::string& path)
    {
        // Check if already loaded
        auto it = m_Assets.find(path);
        if (it != m_Assets.end())
        {
            auto asset = it->second;
            return AssetHandle<T>(asset->GetID(), m_Generations[asset->GetID()]);
        }

        // Create new asset
        auto asset = std::make_shared<T>();
        asset->m_ID = m_NextID++;
        asset->m_Path = path;
        m_Generations[asset->m_ID] = 1;

        // Load the asset
        if (!asset->Load(path))
        {
            GG_CORE_ERROR("Failed to load asset: {}", path);
            return AssetHandle<T>();
        }

        // Store it
        m_Assets[path] = asset;
        m_AssetsByID[asset->m_ID] = asset;

        GG_CORE_TRACE("Loaded asset: {} (ID: {})", path, asset->m_ID);
        return AssetHandle<T>(asset->m_ID, m_Generations[asset->m_ID]);
    }

    template<typename T>
    AssetHandle<T> AssetManager::GetHandle(const std::string& path) const
    {
        auto it = m_Assets.find(path);
        if (it != m_Assets.end())
        {
            auto asset = it->second;
            auto genIt = m_Generations.find(asset->GetID());
            uint32_t gen = (genIt != m_Generations.end()) ? genIt->second : 0;
            return AssetHandle<T>(asset->GetID(), gen);
        }
        return AssetHandle<T>();
    }

    template<typename T>
    T* AssetManager::GetAssetByID(AssetID id) const
    {
        auto it = m_AssetsByID.find(id);
        if (it != m_AssetsByID.end())
        {
            return static_cast<T*>(it->second.get());
        }
        return nullptr;
    }

    // AssetHandle template implementations (need AssetManager definition)

    template<typename T>
    bool AssetHandle<T>::IsValid() const
    {
        if (m_ID == InvalidAssetID)
            return false;

        auto& manager = AssetManager::Get();
        return manager.GetGeneration(m_ID) == m_Generation;
    }

    template<typename T>
    T* AssetHandle<T>::Get() const
    {
        if (!IsValid())
            return nullptr;

        return AssetManager::Get().GetAssetByID<T>(m_ID);
    }

}
