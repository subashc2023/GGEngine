#include "ggpch.h"
#include "GUID.h"

#include <random>
#include <sstream>
#include <iomanip>

namespace GGEngine {

    static std::random_device s_RandomDevice;
    static std::mt19937_64 s_Engine(s_RandomDevice());
    static std::uniform_int_distribution<uint64_t> s_Distribution;

    GUID GUID::Generate()
    {
        GUID guid;
        guid.High = s_Distribution(s_Engine);
        guid.Low = s_Distribution(s_Engine);
        return guid;
    }

    std::string GUID::ToString() const
    {
        std::ostringstream ss;
        ss << std::hex << std::setfill('0')
           << std::setw(16) << High
           << std::setw(16) << Low;
        return ss.str();
    }

    GUID GUID::FromString(const std::string& str)
    {
        GUID guid;
        if (str.length() >= 32)
        {
            guid.High = std::stoull(str.substr(0, 16), nullptr, 16);
            guid.Low = std::stoull(str.substr(16, 16), nullptr, 16);
        }
        return guid;
    }

}
