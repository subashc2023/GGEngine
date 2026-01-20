#include "ggpch.h"
#include "DebugUI.h"
#include "GGEngine/Application.h"

#include <imgui.h>

namespace GGEngine {

    void DebugUI::ShowStats(Timestep ts)
    {
        ImGui::Begin("Stats");
        ShowStatsContent(ts);
        ImGui::End();
    }

    void DebugUI::ShowStatsContent(Timestep ts)
    {
        ImGuiIO& io = ImGui::GetIO();

        // Use ImGui's built-in framerate (calculated from its own delta time)
        ImGui::Text("ImGui FPS: %.1f (%.3f ms)", io.Framerate, 1000.0f / io.Framerate);
        ImGui::Text("Engine FPS: %.1f (%.3f ms)", 1.0f / ts.GetSeconds(), ts.GetMilliseconds());

        ImGui::Separator();

        auto& window = Application::Get().GetWindow();
        bool vsync = window.IsVSync();
        if (ImGui::Checkbox("VSync", &vsync))
        {
            window.SetVSync(vsync);
        }
    }

}
