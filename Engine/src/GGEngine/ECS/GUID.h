#pragma once

#include "GGEngine/Core/Core.h"
#include <cstdint>
#include <string>

namespace GGEngine {

    // 128-bit GUID for persistent asset/entity identification
    struct GG_API GUID
    {
        uint64_t High = 0;
        uint64_t Low = 0;

        bool IsValid() const { return High != 0 || Low != 0; }

        bool operator==(const GUID& other) const
        {
            return High == other.High && Low == other.Low;
        }

        bool operator!=(const GUID& other) const { return !(*this == other); }

        // Generate new random GUID
        static GUID Generate();

        // Convert to/from string for serialization
        std::string ToString() const;
        static GUID FromString(const std::string& str);
    };

    // Hash function for use in unordered_map
    struct GUIDHash
    {
        size_t operator()(const GUID& guid) const
        {
            return std::hash<uint64_t>()(guid.High) ^
                   (std::hash<uint64_t>()(guid.Low) << 1);
        }
    };

}
