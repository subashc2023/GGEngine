#pragma once

#include "GGEngine/Core/Core.h"
#include <cstdint>

namespace GGEngine {

    // Entity is just an index into component arrays
    using Entity = uint32_t;
    constexpr Entity InvalidEntity = UINT32_MAX;

    // Combined entity index + generation for stale reference detection
    struct EntityID
    {
        uint32_t Index = InvalidEntity;
        uint32_t Generation = 0;

        bool IsValid() const { return Index != InvalidEntity; }

        bool operator==(const EntityID& other) const
        {
            return Index == other.Index && Generation == other.Generation;
        }

        bool operator!=(const EntityID& other) const { return !(*this == other); }
    };

    constexpr EntityID InvalidEntityID = { InvalidEntity, 0 };

}
