#pragma once

#include "Core.h"

namespace GGEngine {

    class GG_API Application 
    {
    public:
        Application();
        virtual ~Application();

        void Run();
    };
}