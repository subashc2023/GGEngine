#pragma once

#include "Asset.h"

#include <cstdint>

namespace GGEngine {

    // Forward declaration
    class AssetManager;

    // Lightweight handle to an asset - can be copied freely
    // Uses generation counter to detect stale handles after asset unload
    template<typename T>
    class AssetHandle
    {
    public:
        AssetHandle() = default;

        bool IsValid() const;
        T* Get() const;

        T* operator->() const { return Get(); }
        T& operator*() const { return *Get(); }

        explicit operator bool() const { return IsValid(); }

        AssetID GetID() const { return m_ID; }
        uint32_t GetGeneration() const { return m_Generation; }

    private:
        friend class AssetManager;

        AssetHandle(AssetID id, uint32_t generation)
            : m_ID(id), m_Generation(generation) {}

        AssetID m_ID = InvalidAssetID;
        uint32_t m_Generation = 0;
    };

}
