#pragma once

#include "GGEngine/Core/Core.h"

namespace GGEngine {

    class GG_API Random
    {
    public:
        static void Init();
        static float Float();  // Returns [0.0, 1.0]
    };

}
