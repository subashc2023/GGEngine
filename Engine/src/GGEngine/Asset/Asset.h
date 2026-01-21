#pragma once

#include "GGEngine/Core/Core.h"

#include <string>
#include <cstdint>
#include <filesystem>

namespace GGEngine {

    // Unique identifier for an asset
    using AssetID = uint64_t;
    constexpr AssetID InvalidAssetID = 0;

    // Asset type enumeration for runtime type identification
    enum class AssetType : uint32_t
    {
        None = 0,
        Shader,
        Texture,
        Mesh,
        Audio
    };

    // Base class for all assets
    class GG_API Asset
    {
    public:
        virtual ~Asset() = default;

        virtual AssetType GetType() const = 0;
        virtual void Unload() {}  // Override to release GPU/external resources

        const std::filesystem::path& GetPath() const { return m_Path; }
        AssetID GetID() const { return m_ID; }
        bool IsLoaded() const { return m_Loaded; }

    protected:
        friend class AssetManager;

        std::filesystem::path m_Path;
        AssetID m_ID = InvalidAssetID;
        bool m_Loaded = false;
    };

    // Helper to get AssetType from template type - specialized per asset type
    template<typename T>
    constexpr AssetType GetAssetType() { return AssetType::None; }

}
