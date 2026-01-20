#include "ggpch.h"
#include "AssetManager.h"

#ifdef GG_PLATFORM_WINDOWS
    #include <Windows.h>
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
        UnloadAll();
        GG_CORE_INFO("AssetManager shutdown");
    }

    void AssetManager::DetectAssetRoot()
    {
        // Get executable directory
        std::filesystem::path exePath;

#ifdef GG_PLATFORM_WINDOWS
        char buffer[MAX_PATH];
        GetModuleFileNameA(nullptr, buffer, MAX_PATH);
        exePath = std::filesystem::path(buffer).parent_path();
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
            m_Generations[id]++; // Invalidate handles
            m_AssetsByID.erase(id);
            m_Assets.erase(it);
            GG_CORE_TRACE("Unloaded asset: {}", path);
        }
    }

    void AssetManager::UnloadAll()
    {
        for (auto& [path, asset] : m_Assets)
        {
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
            // Trace level - callers should log errors if file was required
            GG_CORE_TRACE("File not found: {}", absolutePath.string());
            return {};
        }

        size_t fileSize = static_cast<size_t>(file.tellg());
        std::vector<char> buffer(fileSize);

        file.seekg(0);
        file.read(buffer.data(), fileSize);
        file.close();

        return buffer;
    }

}
