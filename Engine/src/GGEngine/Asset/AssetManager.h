#pragma once

#include "GGEngine/Core/Core.h"
#include "Asset.h"
#include "AssetHandle.h"

#include <filesystem>
#include <unordered_map>
#include <string>
#include <vector>
#include <functional>
#include <mutex>
#include <queue>

#ifndef GG_DIST
#include "GGEngine/Utils/FileWatcher.h"
#endif

namespace GGEngine {

    // Forward declarations
    class Texture;
    struct TextureCPUData;

    // Callback types
    using AssetReadyCallback = std::function<void(AssetID id, bool success)>;
    using AssetReloadCallback = std::function<void(AssetID id)>;

    class GG_API AssetManager
    {
    public:
        static AssetManager& Get();

        void Init();
        void Shutdown();

        // Called every frame from main loop - processes pending async loads and callbacks
        void Update();

        // ================================================================
        // Async Loading API
        // ================================================================

        // Load texture asynchronously - returns handle immediately
        // Handle's IsReady() returns false until load completes
        // Use OnAssetReady() to get notified when loading finishes
        AssetHandle<Texture> LoadTextureAsync(const std::string& path);

        // Register callback for when an asset becomes ready (or fails)
        void OnAssetReady(AssetID id, AssetReadyCallback callback);

        // ================================================================
        // Hot Reload API (Debug & Release builds, excluded from Dist)
        // ================================================================
#ifndef GG_DIST
        // Enable/disable automatic hot reload for assets in watched directories
        void EnableHotReload(bool enable);
        bool IsHotReloadEnabled() const { return m_HotReloadEnabled; }

        // Watch a directory for file changes (relative to asset root)
        void WatchDirectory(const std::string& relativePath);

        // Register callback for when an asset is reloaded
        void OnAssetReload(AssetID id, AssetReloadCallback callback);
#endif

        // ================================================================

        // Set custom asset root (optional - auto-detected if not set)
        void SetAssetRoot(const std::filesystem::path& root);
        const std::filesystem::path& GetAssetRoot() const { return m_AssetRoot; }

        // Add additional search path for assets (relative to asset root)
        // Use this for app-specific asset directories (e.g., "Sandbox/assets", "Editor/assets")
        void AddSearchPath(const std::string& path);

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
        void ProcessPendingTextureUploads();
        void FireReadyCallbacks(AssetID id, bool success);

#ifndef GG_DIST
        void ProcessFileChanges();
        void OnFileChanged(const std::filesystem::path& path, FileChangeType type);
        void FireReloadCallbacks(AssetID id);
#endif

        std::filesystem::path m_AssetRoot;
        std::vector<std::string> m_SearchPaths;  // Additional search paths (relative to root)
        std::unordered_map<std::string, Ref<Asset>> m_Assets;
        std::unordered_map<AssetID, Ref<Asset>> m_AssetsByID;
        std::unordered_map<AssetID, uint32_t> m_Generations;
        AssetID m_NextID = 1;

        // ================================================================
        // Async Loading State
        // ================================================================

        // Pending texture uploads (CPU data ready, waiting for GPU upload)
        struct PendingTextureUpload
        {
            AssetID assetId;
            std::unique_ptr<TextureCPUData> cpuData;
        };
        std::queue<PendingTextureUpload> m_PendingTextureUploads;
        std::mutex m_PendingUploadsMutex;

        // Asset ready callbacks
        std::unordered_map<AssetID, std::vector<AssetReadyCallback>> m_ReadyCallbacks;
        std::mutex m_CallbacksMutex;

        // ================================================================
        // Hot Reload State (Debug & Release, excluded from Dist)
        // ================================================================
#ifndef GG_DIST
        FileWatcher m_FileWatcher;
        bool m_HotReloadEnabled = false;

        // Map from absolute file path to asset path for quick lookup during file changes
        std::unordered_map<std::string, std::string> m_AbsoluteToAssetPath;

        // Pending reloads (file path -> time of change, for debouncing)
        std::unordered_map<std::string, std::chrono::steady_clock::time_point> m_PendingReloads;
        std::chrono::milliseconds m_ReloadDebounceTime{100};  // Wait 100ms after last change

        // Asset reload callbacks
        std::unordered_map<AssetID, std::vector<AssetReloadCallback>> m_ReloadCallbacks;
#endif
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
        auto asset = CreateRef<T>();
        asset->m_ID = m_NextID++;
        asset->m_Path = path;
        m_Generations[asset->m_ID] = 1;

        // Load the asset
        auto result = asset->Load(path);
        if (result.IsErr())
        {
            asset->SetError(result.Error());
            GG_CORE_ERROR("Failed to load asset '{}': {}", path, result.Error());
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
