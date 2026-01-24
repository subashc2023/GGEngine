#include "ggpch.h"
#include "AssetManager.h"
#include "Texture.h"
#include "GGEngine/Core/JobSystem.h"

#include <algorithm>
#include <chrono>

// Platform-specific includes for executable path detection
#ifdef GG_PLATFORM_WINDOWS
    #include <Windows.h>
#elif defined(GG_PLATFORM_LINUX)
    #include <unistd.h>
    #include <linux/limits.h>
#elif defined(GG_PLATFORM_MACOS)
    #include <mach-o/dyld.h>
    #include <limits.h>
#endif

namespace GGEngine {

    AssetManager& AssetManager::Get()
    {
        static AssetManager instance;
        return instance;
    }

    void AssetManager::Init()
    {
        DetectAssetRoot();
        GG_CORE_INFO("AssetManager initialized with root: {}", m_AssetRoot.string());
    }

    void AssetManager::Shutdown()
    {
        // Shutdown fallback textures before unloading assets
        Texture::ShutdownFallback();

        UnloadAll();
        GG_CORE_INFO("AssetManager shutdown");
    }

    void AssetManager::Update()
    {
        // Process pending texture uploads (CPU data -> GPU)
        ProcessPendingTextureUploads();

#ifndef GG_DIST
        // Process file changes for hot reload
        if (m_HotReloadEnabled)
        {
            ProcessFileChanges();
        }
#endif
    }

    AssetHandle<Texture> AssetManager::LoadTextureAsync(const std::string& path)
    {
        // Check if already loaded or loading
        auto it = m_Assets.find(path);
        if (it != m_Assets.end())
        {
            auto asset = it->second;
            return AssetHandle<Texture>(asset->GetID(), m_Generations[asset->GetID()]);
        }

        // Create the asset immediately (in Loading state)
        auto texture = CreateRef<Texture>();
        texture->m_ID = m_NextID++;
        texture->m_Path = path;
        texture->SetState(AssetState::Loading);
        m_Generations[texture->m_ID] = 1;

        // Store it before submitting job (so handle is valid immediately)
        m_Assets[path] = texture;
        m_AssetsByID[texture->m_ID] = texture;

        AssetID assetId = texture->m_ID;
        AssetHandle<Texture> handle(assetId, m_Generations[assetId]);

        GG_CORE_TRACE("Async texture load started: {} (ID: {})", path, assetId);

        // Submit job to load CPU data on worker thread
        JobSystem::Get().Submit(
            [this, path, assetId]() {
                // Worker thread: Load texture data to CPU
                TextureCPUData cpuData = Texture::LoadCPU(path);

                // Queue for GPU upload on main thread
                auto cpuDataPtr = std::make_unique<TextureCPUData>(std::move(cpuData));

                {
                    std::lock_guard<std::mutex> lock(m_PendingUploadsMutex);
                    m_PendingTextureUploads.push({assetId, std::move(cpuDataPtr)});
                }
            }
        );

        return handle;
    }

    void AssetManager::OnAssetReady(AssetID id, AssetReadyCallback callback)
    {
        if (!callback) return;

        // Check if asset is already ready
        auto it = m_AssetsByID.find(id);
        if (it != m_AssetsByID.end() && it->second->IsReady())
        {
            // Already ready, call immediately
            callback(id, true);
            return;
        }

        // Register callback for later
        std::lock_guard<std::mutex> lock(m_CallbacksMutex);
        m_ReadyCallbacks[id].push_back(std::move(callback));
    }

    void AssetManager::ProcessPendingTextureUploads()
    {
        // Swap out pending uploads to minimize lock time
        std::queue<PendingTextureUpload> uploadsToProcess;
        {
            std::lock_guard<std::mutex> lock(m_PendingUploadsMutex);
            std::swap(uploadsToProcess, m_PendingTextureUploads);
        }

        while (!uploadsToProcess.empty())
        {
            auto& upload = uploadsToProcess.front();

            // Find the texture asset
            auto it = m_AssetsByID.find(upload.assetId);
            if (it == m_AssetsByID.end())
            {
                GG_CORE_WARN("Async texture upload: asset {} no longer exists", upload.assetId);
                uploadsToProcess.pop();
                continue;
            }

            Texture* texture = static_cast<Texture*>(it->second.get());

            bool success = false;
            if (upload.cpuData && upload.cpuData->IsValid())
            {
                // Upload to GPU
                texture->SetState(AssetState::Uploading);
                success = texture->UploadGPU(std::move(*upload.cpuData));
            }
            else
            {
                // CPU load failed
                texture->SetError("Failed to load texture data");
                GG_CORE_ERROR("Async texture load failed for asset {}", upload.assetId);
            }

            // Fire callbacks
            FireReadyCallbacks(upload.assetId, success);

            uploadsToProcess.pop();
        }
    }

    void AssetManager::FireReadyCallbacks(AssetID id, bool success)
    {
        std::vector<AssetReadyCallback> callbacks;
        {
            std::lock_guard<std::mutex> lock(m_CallbacksMutex);
            auto it = m_ReadyCallbacks.find(id);
            if (it != m_ReadyCallbacks.end())
            {
                callbacks = std::move(it->second);
                m_ReadyCallbacks.erase(it);
            }
        }

        for (auto& callback : callbacks)
        {
            if (callback)
                callback(id, success);
        }
    }

    void AssetManager::DetectAssetRoot()
    {
        // Get executable directory (platform-specific)
        std::filesystem::path exePath;

#ifdef GG_PLATFORM_WINDOWS
        char buffer[MAX_PATH];
        GetModuleFileNameA(nullptr, buffer, MAX_PATH);
        exePath = std::filesystem::path(buffer).parent_path();
#elif defined(GG_PLATFORM_LINUX)
        char buffer[PATH_MAX];
        ssize_t len = readlink("/proc/self/exe", buffer, sizeof(buffer) - 1);
        if (len != -1)
        {
            buffer[len] = '\0';
            exePath = std::filesystem::path(buffer).parent_path();
        }
        else
        {
            GG_CORE_WARN("Failed to get executable path, using current directory");
            exePath = std::filesystem::current_path();
        }
#elif defined(GG_PLATFORM_MACOS)
        char buffer[PATH_MAX];
        uint32_t size = sizeof(buffer);
        if (_NSGetExecutablePath(buffer, &size) == 0)
        {
            // Resolve symlinks and get canonical path
            char realPath[PATH_MAX];
            if (realpath(buffer, realPath) != nullptr)
            {
                exePath = std::filesystem::path(realPath).parent_path();
            }
            else
            {
                exePath = std::filesystem::path(buffer).parent_path();
            }
        }
        else
        {
            GG_CORE_WARN("Failed to get executable path, using current directory");
            exePath = std::filesystem::current_path();
        }
#else
        // Fallback for other platforms
        exePath = std::filesystem::current_path();
#endif

#ifdef GG_DIST
        // In Dist builds, assets are alongside executable
        m_AssetRoot = exePath;
        GG_CORE_TRACE("Dist build: using executable directory for assets");
#else
        // In Debug/Release, try to find source assets directory
        // Walk up from exe looking for "Engine/assets"
        std::filesystem::path searchPath = exePath;
        for (int i = 0; i < 6; ++i) // Max 6 levels up
        {
            if (std::filesystem::exists(searchPath / "Engine" / "assets"))
            {
                m_AssetRoot = searchPath;
                GG_CORE_TRACE("Found project root at: {}", m_AssetRoot.string());
                return;
            }

            auto parent = searchPath.parent_path();
            if (parent == searchPath) // Hit filesystem root
                break;
            searchPath = parent;
        }

        // Fallback to executable directory
        m_AssetRoot = exePath;
        GG_CORE_WARN("Could not find project root, using executable directory");
#endif
    }

    void AssetManager::SetAssetRoot(const std::filesystem::path& root)
    {
        m_AssetRoot = root;
        GG_CORE_INFO("Asset root set to: {}", m_AssetRoot.string());
    }

    void AssetManager::AddSearchPath(const std::string& path)
    {
        // Avoid duplicates
        for (const auto& existing : m_SearchPaths)
        {
            if (existing == path)
                return;
        }
        m_SearchPaths.push_back(path);
        GG_CORE_INFO("Added asset search path: {}", path);
    }

    std::filesystem::path AssetManager::ResolvePath(const std::string& relativePath) const
    {
        std::filesystem::path relPath(relativePath);

        // Try direct path first
        std::filesystem::path fullPath = m_AssetRoot / relPath;
        if (std::filesystem::exists(fullPath))
            return fullPath;

        // Try with Engine/ prefix (for source builds)
        fullPath = m_AssetRoot / "Engine" / relPath;
        if (std::filesystem::exists(fullPath))
            return fullPath;

        // Try each additional search path
        for (const auto& searchPath : m_SearchPaths)
        {
            fullPath = m_AssetRoot / searchPath / relPath;
            if (std::filesystem::exists(fullPath))
                return fullPath;
        }

        // Return the first path (will fail with good error message)
        return m_AssetRoot / relPath;
    }

    uint32_t AssetManager::GetGeneration(AssetID id) const
    {
        auto it = m_Generations.find(id);
        if (it != m_Generations.end())
            return it->second;
        return 0;
    }

    bool AssetManager::IsLoaded(const std::string& path) const
    {
        return m_Assets.find(path) != m_Assets.end();
    }

    void AssetManager::Unload(const std::string& path)
    {
        auto it = m_Assets.find(path);
        if (it != m_Assets.end())
        {
            AssetID id = it->second->GetID();
            it->second->Unload();  // Release GPU/external resources
            m_Generations[id]++;   // Invalidate handles
            m_AssetsByID.erase(id);
            m_Assets.erase(it);
            GG_CORE_TRACE("Unloaded asset: {}", path);
        }
    }

    void AssetManager::UnloadAll()
    {
        // Explicitly unload all assets while Vulkan is still valid
        for (auto& [path, asset] : m_Assets)
        {
            asset->Unload();
            m_Generations[asset->GetID()]++;
        }
        m_Assets.clear();
        m_AssetsByID.clear();
        GG_CORE_TRACE("Unloaded all assets");
    }

    std::vector<char> AssetManager::ReadFileRaw(const std::string& relativePath) const
    {
        return ReadFileRawAbsolute(ResolvePath(relativePath));
    }

    std::vector<char> AssetManager::ReadFileRawAbsolute(const std::filesystem::path& absolutePath) const
    {
        std::ifstream file(absolutePath, std::ios::ate | std::ios::binary);

        if (!file.is_open())
        {
            // Silent - callers should log errors if file was required
            return {};
        }

        size_t fileSize = static_cast<size_t>(file.tellg());
        std::vector<char> buffer(fileSize);

        file.seekg(0);
        file.read(buffer.data(), fileSize);
        file.close();

        return buffer;
    }

    // ========================================================================
    // Hot Reload Implementation (Debug & Release, excluded from Dist)
    // ========================================================================

#ifndef GG_DIST
    void AssetManager::EnableHotReload(bool enable)
    {
        if (m_HotReloadEnabled == enable)
            return;

        m_HotReloadEnabled = enable;
        m_FileWatcher.SetEnabled(enable);

        if (enable)
        {
            GG_CORE_INFO("Hot reload enabled");
        }
        else
        {
            GG_CORE_INFO("Hot reload disabled");
        }
    }

    void AssetManager::WatchDirectory(const std::string& relativePath)
    {
        std::filesystem::path fullPath = ResolvePath(relativePath);
        if (!std::filesystem::exists(fullPath))
        {
            GG_CORE_WARN("AssetManager::WatchDirectory - directory does not exist: {}", relativePath);
            return;
        }

        // Set up file watch callback
        m_FileWatcher.Watch(fullPath, [this](const std::filesystem::path& path, FileChangeType type) {
            OnFileChanged(path, type);
        });
    }

    void AssetManager::OnAssetReload(AssetID id, AssetReloadCallback callback)
    {
        if (!callback) return;

        m_ReloadCallbacks[id].push_back(std::move(callback));
    }

    void AssetManager::OnFileChanged(const std::filesystem::path& changedPath, FileChangeType type)
    {
        // Only care about modifications (not creates/deletes for hot reload)
        if (type != FileChangeType::Modified)
            return;

        std::string pathStr = changedPath.string();

        // Check if this is a known asset file extension
        std::string ext = changedPath.extension().string();
        std::transform(ext.begin(), ext.end(), ext.begin(), ::tolower);

        // Only reload supported texture formats
        if (ext != ".png" && ext != ".jpg" && ext != ".jpeg" &&
            ext != ".bmp" && ext != ".tga" && ext != ".gif")
        {
            return;
        }

        // Add to pending reloads with debounce time
        m_PendingReloads[pathStr] = std::chrono::steady_clock::now();

        GG_CORE_TRACE("File change detected: {} (queued for reload)", changedPath.filename().string());
    }

    void AssetManager::ProcessFileChanges()
    {
        // Poll file watcher
        m_FileWatcher.Update();

        // Process debounced reloads
        auto now = std::chrono::steady_clock::now();
        std::vector<std::string> readyToReload;

        for (auto& [path, lastChange] : m_PendingReloads)
        {
            auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(now - lastChange);
            if (elapsed >= m_ReloadDebounceTime)
            {
                readyToReload.push_back(path);
            }
        }

        for (const auto& absolutePath : readyToReload)
        {
            m_PendingReloads.erase(absolutePath);

            // Find matching asset by comparing resolved paths
            std::filesystem::path changedPath(absolutePath);

            for (auto& [assetPath, asset] : m_Assets)
            {
                // Skip non-texture assets
                if (asset->GetType() != AssetType::Texture)
                    continue;

                Texture* texture = static_cast<Texture*>(asset.get());
                std::filesystem::path resolvedAssetPath = ResolvePath(texture->GetSourcePath());

                // Compare canonical paths
                try
                {
                    auto canonicalChanged = std::filesystem::canonical(changedPath);
                    auto canonicalAsset = std::filesystem::canonical(resolvedAssetPath);

                    if (canonicalChanged == canonicalAsset)
                    {
                        GG_CORE_INFO("Hot reload triggered for: {}", assetPath);

                        if (texture->Reload())
                        {
                            // Fire reload callbacks
                            FireReloadCallbacks(asset->GetID());
                        }
                        break;  // Found the matching asset
                    }
                }
                catch (const std::filesystem::filesystem_error& e)
                {
                    // Path doesn't exist or access denied - skip
                    GG_CORE_TRACE("Hot reload path comparison failed: {}", e.what());
                }
            }
        }
    }

    void AssetManager::FireReloadCallbacks(AssetID id)
    {
        auto it = m_ReloadCallbacks.find(id);
        if (it == m_ReloadCallbacks.end())
            return;

        for (auto& callback : it->second)
        {
            if (callback)
                callback(id);
        }
    }
#endif

}
