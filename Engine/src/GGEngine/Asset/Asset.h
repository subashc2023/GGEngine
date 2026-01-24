#pragma once

#include "GGEngine/Core/Core.h"

#include <string>
#include <cstdint>
#include <filesystem>
#include <atomic>

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
        Audio,
        Material
    };

    // Asset loading state for async support
    enum class AssetState : uint8_t
    {
        Unloaded = 0,   // Not loaded, or explicitly unloaded
        Loading,        // Being loaded asynchronously (CPU work)
        Uploading,      // CPU data ready, waiting for GPU upload
        Ready,          // Fully loaded and usable
        Failed,         // Load failed (check GetErrorMessage())
        Reloading       // Hot reload in progress (existing data still usable)
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

        // State accessors
        AssetState GetState() const { return m_State.load(std::memory_order_acquire); }
        bool IsReady() const { return GetState() == AssetState::Ready; }
        bool IsLoading() const
        {
            AssetState s = GetState();
            return s == AssetState::Loading || s == AssetState::Uploading;
        }
        bool IsFailed() const { return GetState() == AssetState::Failed; }

        // Backwards compatibility - maps to Ready state
        bool IsLoaded() const { return IsReady(); }

        // Error message for failed loads
        const std::string& GetErrorMessage() const { return m_ErrorMessage; }

    protected:
        friend class AssetManager;

        // Set state (thread-safe)
        void SetState(AssetState state) { m_State.store(state, std::memory_order_release); }
        void SetError(const std::string& message)
        {
            m_ErrorMessage = message;
            SetState(AssetState::Failed);
        }

        std::filesystem::path m_Path;
        AssetID m_ID = InvalidAssetID;
        std::atomic<AssetState> m_State{AssetState::Unloaded};
        std::string m_ErrorMessage;
    };

    // Helper to get AssetType from template type - specialized per asset type
    template<typename T>
    constexpr AssetType GetAssetType() { return AssetType::None; }

}
