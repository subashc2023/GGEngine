#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Core/Timestep.h"

namespace GGEngine {

    class GG_API DebugUI
    {
    public:
        // Renders a stats window with FPS, frame time, and VSync toggle
        // Call this from any layer's OnUpdate()
        static void ShowStats(Timestep ts);

        // Renders just the stats content (no window) - use inside your own ImGui::Begin/End
        static void ShowStatsContent(Timestep ts);
    };

}
