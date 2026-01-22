#pragma once

#include "GGEngine/Core/Core.h"
#include <string>

namespace GGEngine {

    class GG_API FileDialogs
    {
    public:
        // Returns empty string if cancelled
        static std::string OpenFile(const char* filter, const char* title = "Open File");
        static std::string SaveFile(const char* filter, const char* title = "Save File");
    };

}
